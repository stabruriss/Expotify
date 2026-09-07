import { query } from "@anthropic-ai/claude-agent-sdk";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { bootstrapObserver, guardSdkMessage, isClaudeAccount, isolatedEnvironment, normalizeCatalog, queryOptions, redactError, requireClaudeAccount } from "./protocol.mjs";

const cliPath = path.join(path.dirname(process.execPath), "claude");

async function readRequest() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > 4 * 1024 * 1024) throw new Error("Claude request is too large");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
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

async function main() {
  const request = await readRequest();
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
    case "prompt": return runPrompt(request, env);
    default: throw new Error("Unknown Claude runtime action");
  }
}

main().then(data => {
  process.stdout.write(`${JSON.stringify({ ok: true, data })}\n`);
}).catch(error => {
  process.stdout.write(`${JSON.stringify({ ok: false, error: redactError(error), code: error?.code || "request_failed" })}\n`);
  process.exitCode = 1;
});
