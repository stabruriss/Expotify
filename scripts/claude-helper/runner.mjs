import { query } from "@anthropic-ai/claude-agent-sdk";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { bootstrapObserver, buildToolServer, createToolBridge, guardSdkMessage, isClaudeAccount, isolatedEnvironment, nativeQueryOptions, normalizeCatalog, queryOptions, redactError, requireClaudeAccount } from "./protocol.mjs";

const cliPath = path.join(path.dirname(process.execPath), "claude");
const MAX_LINE_BYTES = 4 * 1024 * 1024;

// stdin is a line channel: the first line is the request; in native tool-calling mode the
// Rust side keeps it open and answers tool calls with further lines.
function createStdinLines() {
  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity, terminal: false });
  const buffered = [];
  const closeHandlers = [];
  let handler = null;
  let closed = false;
  rl.on("line", (line) => { if (handler) handler(line); else buffered.push(line); });
  rl.on("close", () => { closed = true; for (const fn of closeHandlers.splice(0)) fn(); });
  return {
    next() {
      return new Promise((resolve, reject) => {
        if (buffered.length) return resolve(buffered.shift());
        if (closed) return reject(new Error("Claude request channel closed before a request arrived"));
        handler = (line) => { handler = null; resolve(line); };
        closeHandlers.push(() => reject(new Error("Claude request channel closed before a request arrived")));
      });
    },
    onLine(fn) { handler = fn; for (const line of buffered.splice(0)) fn(line); },
    onClose(fn) { if (closed) fn(); else closeHandlers.push(fn); },
    close() { rl.close(); },
  };
}

async function readRequest(stdin) {
  const line = await stdin.next();
  if (line.length > MAX_LINE_BYTES) throw new Error("Claude request is too large");
  return JSON.parse(line);
}

function runCli(args, env) {
  return new Promise((resolve, reject) => {
    const child = spawn(cliPath, args, { env, cwd: env.CLAUDE_CONFIG_DIR, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const capture = stream => data => {
      if (stream === "stdout") stdout += data;
      else stderr += data;
      if (stdout.length + stderr.length > 2 * 1024 * 1024) {
        child.kill("SIGKILL");
        reject(new Error("Claude runtime output exceeded its limit"));
      }
    };
    child.stdout.on("data", capture("stdout"));
    child.stderr.on("data", capture("stderr"));
    child.on("error", reject);
    child.on("close", code => resolve({ code, stdout, stderr }));
  });
}

async function nativeAccount(env) {
  const result = await runCli(["auth", "status", "--json"], env);
  let data;
  try { data = JSON.parse(result.stdout); }
  catch { throw new Error("Could not read Claude sign-in status"); }
  if (typeof data.loggedIn !== "boolean") throw new Error("Invalid Claude sign-in status");
  return { loggedIn: data.loggedIn, authMethod: data.authMethod, apiProvider: data.apiProvider, subscriptionType: data.subscriptionType };
}

async function metadata(request, env, observer) {
  let release;
  const waiting = new Promise(resolve => { release = resolve; });
  // Initialize the SDK without sending a generation request.
  const session = query({
    prompt: (async function* () { await waiting; })(),
    options: { ...queryOptions(request, cliPath, env),
      // Metadata only. Capture in memory; never write account diagnostics to disk.
      ...(observer ? { debugFile: "/dev/stderr", stderr: chunk => observer.observe(chunk) } : {}),
    },
  });
  const close = () => { release(); session.close(); };
  try {
    const rows = await session.supportedModels();
    return { rows, close };
  } catch (error) { close(); throw error; }
}

async function catalog(request, env) {
  const account = await nativeAccount(env);
  requireClaudeAccount(account);
  const started = Date.now();
  let answeredAt = 0;
  const observer = bootstrapObserver();
  const warmup = await metadata(request, env, observer);
  try {
    // The pinned CLI refreshes its bootstrap cache after initialization. Its
    // model list is a startup snapshot, so read it in a second process.
    while (Date.now() - started < 6500) {
      try {
        const config = JSON.parse(await readFile(path.join(request.configDir, ".claude.json"), "utf8"));
        const answered = config.additionalModelOptionsAnsweredAt;
        const timestamp = typeof answered === "number" ? answered : Date.parse(answered);
        if (Number.isFinite(timestamp)) answeredAt = timestamp;
        if (observer.isFresh(answeredAt, started)) break;
      } catch { /* A new account may not have a bootstrap cache yet. */ }
      await delay(150);
    }
  } finally { warmup.close(); }
  const snapshot = await metadata(request, env);
  try {
    const fresh = observer.isFresh(answeredAt, started);
    return { ...normalizeCatalog(snapshot.rows, account),
      fetched_at: answeredAt ? new Date(fresh ? Date.now() : answeredAt).toISOString() : null,
      stale: !fresh,
      error: !fresh ? "Claude bootstrap refresh did not complete; using the runtime's cached catalog" : null,
    };
  }
  finally { snapshot.close(); }
}

async function sdkProbe() {
  // Packaging check only: never initialize against a caller's home or account.
  const configDir = await mkdtemp(path.join(os.tmpdir(), "expotify-claude-probe-"));
  const env = {
    ...isolatedEnvironment(configDir, { HOME: configDir }),
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  };
  try {
    await mkdir(path.join(configDir, "workspace"), { mode: 0o700 });
    if ((await nativeAccount(env)).loggedIn) throw new Error("SDK probe requires an isolated signed-out account");
    const snapshot = await metadata({ configDir }, env);
    try {
      if (!Array.isArray(snapshot.rows) || snapshot.rows.length === 0) {
        throw new Error("SDK initialization did not return model metadata");
      }
      return { sdkInitialized: true };
    } finally { snapshot.close(); }
  } finally { await rm(configDir, { recursive: true, force: true }); }
}

async function runPrompt(request, env) {
  if (typeof request.prompt !== "string" || !request.prompt.trim()) throw new Error("Claude prompt is missing");
  // Do not allow the runtime to fall through to a global Console/API-key profile.
  requireClaudeAccount(await nativeAccount(env));
  const session = query({ prompt: request.prompt, options: queryOptions(request, cliPath, env) });
  let text = "";
  let completed = false;
  try {
    for await (const message of session) {
      guardSdkMessage(message);
      if (message.type === "assistant") {
        if (message.error) throw new Error(`Claude: ${message.error}`);
        for (const block of message.message.content) {
          if (block.type === "text") text += block.text;
        }
      }
      if (message.type === "result") {
        if (message.subtype !== "success" || message.is_error) {
          throw new Error(message.errors?.join("; ") || "Claude request failed");
        }
        completed = true;
        if (typeof message.result === "string") text = message.result;
      }
    }
  } finally { session.close(); }
  if (!completed || !text.trim()) throw new Error("Claude returned an incomplete or empty response");
  return { text: text.trim() };
}

// Native tool calling: tools are defined by the Rust registry (request.tools, JSON Schema);
// every call is relayed to Rust over stdout/stdin and executed there. Plain assistant text
// is never interpreted as an action on this path.
async function runNativePrompt(request, env, stdin) {
  if (typeof request.prompt !== "string" || !request.prompt.trim()) throw new Error("Claude prompt is missing");
  if (!Array.isArray(request.tools) || request.tools.length === 0) throw new Error("Native tool calling requires tool definitions");
  requireClaudeAccount(await nativeAccount(env));
  const bridge = createToolBridge((line) => process.stdout.write(line));
  stdin.onLine((line) => bridge.feed(line));
  stdin.onClose(() => bridge.close("Expotify closed the tool channel"));
  let bridgeFailure = null;
  const server = buildToolServer(request.tools, async (call) => {
    try {
      return await bridge.call(call);
    } catch (error) {
      bridgeFailure ??= error;
      return { ok: false, output: "Tool execution is unavailable; stop and tell the user." };
    }
  });
  const session = query({ prompt: request.prompt, options: nativeQueryOptions(request, cliPath, env, server) });
  let text = "";
  let completed = false;
  let turns = null;
  let toolUses = 0;
  try {
    for await (const message of session) {
      guardSdkMessage(message);
      if (bridgeFailure) throw bridgeFailure;
      if (message.type === "assistant") {
        if (message.error) throw new Error(`Claude: ${message.error}`);
        for (const block of message.message?.content ?? []) {
          if (block.type === "tool_use") toolUses++;
        }
      }
      if (message.type === "result") {
        turns = Number.isInteger(message.num_turns) ? message.num_turns : null;
        if (message.subtype !== "success" || message.is_error) {
          throw new Error(message.errors?.join("; ") || `Claude request failed (${message.subtype})`);
        }
        completed = true;
        if (typeof message.result === "string") text = message.result;
      }
    }
  } finally {
    session.close();
    bridge.close("session ended");
  }
  if (!completed) throw new Error("Claude returned an incomplete response");
  if (!text.trim() && toolUses === 0) throw new Error("Claude returned an empty response");
  return { text: text.trim(), turns, tool_uses: toolUses };
}

async function main(stdin) {
  const request = await readRequest(stdin);
  if (request.action === "probe") return sdkProbe();
  const env = isolatedEnvironment(request.configDir);
  await mkdir(path.join(request.configDir, "workspace"), { recursive: true, mode: 0o700 });
  switch (request.action) {
    case "status": return { loggedIn: isClaudeAccount(await nativeAccount(env)) };
    case "login": {
      const result = await runCli(["auth", "login", "--claudeai"], env);
      if (result.code !== 0) throw new Error("Claude sign-in was not completed. Please reconnect.");
      requireClaudeAccount(await nativeAccount(env));
      return { loggedIn: true };
    }
    case "logout": {
      const result = await runCli(["auth", "logout"], env);
      if (result.code !== 0) throw new Error("Claude sign-out failed");
      return { loggedIn: false };
    }
    case "catalog": return catalog(request, env);
    case "prompt": return request.protocol === "native" ? runNativePrompt(request, env, stdin) : runPrompt(request, env);
    default: throw new Error("Unknown Claude runtime action");
  }
}

const stdinLines = createStdinLines();
main(stdinLines).then(data => {
  finish(`${JSON.stringify({ ok: true, data })}\n`, 0);
}).catch(error => {
  finish(`${JSON.stringify({ ok: false, error: redactError(error), code: error?.code || "request_failed" })}\n`, 1);
});

// The open stdin line channel would keep the process alive; exit once the final line is flushed.
function finish(line, code) {
  stdinLines.close();
  process.stdout.write(line, () => process.exit(code));
}
