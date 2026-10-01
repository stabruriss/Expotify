import path from "node:path";
import { z } from "zod";
import { tool, createSdkMcpServer } from "@anthropic-ai/claude-agent-sdk";
import packageInfo from "../../package.json" with { type: "json" };

const SYSTEM_ENV = [
  "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL",
  "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
  "http_proxy", "https_proxy", "all_proxy", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR",
];

export function isolatedEnvironment(configDir, source = process.env) {
  if (!path.isAbsolute(configDir)) throw new Error("Claude configuration directory must be absolute");
  const env = Object.fromEntries(SYSTEM_ENV.filter(key => source[key]).map(key => [key, source[key]]));
  return {
    ...env,
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
    CLAUDE_CONFIG_DIR: configDir,
    CLAUDE_AGENT_SDK_CLIENT_APP: `expotify/${packageInfo.version}`,
    DISABLE_TELEMETRY: "1",
    DISABLE_ERROR_REPORTING: "1",
    DISABLE_AUTOUPDATER: "1",
  };
}

export function queryOptions(request, cliPath, env) {
  return {
    ...(request.model ? { model: request.model } : {}),
    systemPrompt: request.systemPrompt,
    maxTurns: 1,
    tools: [],
    allowedTools: [],
    permissionMode: "dontAsk",
    persistSession: false,
    settingSources: [],
    strictMcpConfig: true,
    cwd: path.join(request.configDir, "workspace"),
    pathToClaudeCodeExecutable: cliPath,
    env,
  };
}

// ---------------------------------------------------------------------------
// Native tool calling: the Rust side owns the tool registry (JSON Schema) and the
// executor. The helper only converts the schema for the SDK and relays each call
// over a line protocol: helper -> stdout `{"type":"tool_call",id,name,args}`,
// Rust -> stdin `{"type":"tool_result",id,ok,output}`. No tool logic lives here.
// ---------------------------------------------------------------------------

/** Convert the registry's strict JSON Schema subset (object of string/integer/number
 *  properties with optional enum/minimum/maximum/description) into a Zod raw shape. */
export function zodShapeFromJsonSchema(schema) {
  if (!schema || schema.type !== "object") throw new Error("tool parameters must be an object schema");
  const required = new Set(schema.required ?? []);
  const shape = {};
  for (const [key, prop] of Object.entries(schema.properties ?? {})) {
    let field;
    if (Array.isArray(prop.enum)) field = z.enum(prop.enum);
    else if (prop.type === "string") field = z.string();
    else if (prop.type === "integer" || prop.type === "number") {
      field = z.number();
      if (prop.type === "integer") field = field.int();
      if (typeof prop.minimum === "number") field = field.min(prop.minimum);
      if (typeof prop.maximum === "number") field = field.max(prop.maximum);
    } else if (prop.type === "boolean") field = z.boolean();
    else throw new Error(`unsupported parameter type for ${key}: ${prop.type}`);
    if (typeof prop.description === "string") field = field.describe(prop.description);
    shape[key] = required.has(key) ? field : field.optional();
  }
  return shape;
}

/** Build the in-process MCP server. `dispatch({name,args})` must resolve to {ok, output}. */
export function buildToolServer(definitions, dispatch) {
  const tools = definitions.map((def) =>
    tool(def.name, def.description, zodShapeFromJsonSchema(def.parameters), async (args) => {
      const result = await dispatch({ name: def.name, args });
      return { content: [{ type: "text", text: String(result.output ?? "") }], isError: !result.ok };
    }, { annotations: { readOnlyHint: false } }),
  );
  return createSdkMcpServer({ name: "expotify", version: "1.0.0", tools, alwaysLoad: true });
}

export function nativeQueryOptions(request, cliPath, env, server) {
  const base = queryOptions(request, cliPath, env);
  return {
    ...base,
    env: { ...env, ENABLE_TOOL_SEARCH: "false" },
    maxTurns: Number.isInteger(request.maxTurns) && request.maxTurns > 0 ? request.maxTurns : 4,
    mcpServers: { expotify: server },
    allowedTools: ["mcp__expotify__*"],
  };
}

/** Pending-call table for the line protocol. `write(line)` sends to Rust; `feed(line)`
 *  receives from Rust; `close(reason)` fails every pending call (Rust went away). */
export function createToolBridge(write) {
  const pending = new Map();
  let sequence = 0;
  let closed = null;
  return {
    get pendingCount() { return pending.size; },
    call({ name, args }) {
      if (closed) return Promise.reject(closed);
      const id = `call-${++sequence}`;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        write(JSON.stringify({ type: "tool_call", id, name, args }) + "\n");
      });
    },
    feed(line) {
      let message;
      try { message = JSON.parse(line); } catch { return false; }
      if (message?.type !== "tool_result" || typeof message.id !== "string") return false;
      const waiter = pending.get(message.id);
      if (!waiter) return false;
      pending.delete(message.id);
      waiter.resolve({ ok: message.ok === true, output: typeof message.output === "string" ? message.output : "" });
      return true;
    },
    close(reason = "tool bridge closed") {
      closed = new Error(reason);
      for (const waiter of pending.values()) waiter.reject(closed);
      pending.clear();
    },
  };
}

export function bootstrapObserver() {
  let tail = "";
  let unchanged = false;
  return {
    observe(chunk) {
      tail = (tail + chunk).slice(-4096);
      unchanged ||= tail.includes("[Bootstrap] Cache unchanged, skipping write");
    },
    isFresh(answeredAt, started) { return answeredAt >= started || (answeredAt > 0 && unchanged); },
  };
}

function modelName(id, fallback) {
  const match = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$/.exec(id);
  if (!match) return fallback || id;
  const [, family, major, minor, context] = match;
  return `${family[0].toUpperCase()}${family.slice(1)} ${major}${minor ? `.${minor}` : ""}${context ? " (1M)" : ""}`;
}

export function normalizeCatalog(rows, account) {
  requireClaudeAccount(account);
  const defaultModel = rows.find(row => row.value === "default")?.resolvedModel;
  if (!defaultModel?.startsWith("claude-")) {
    throw new Error("Claude runtime did not provide a resolved default model");
  }
  const models = new Map();
  for (const row of rows) {
    const id = row.resolvedModel || (row.value?.startsWith("claude-") ? row.value : null);
    if (typeof id === "string" && id.startsWith("claude-")) {
      models.set(id, { id, name: modelName(id, row.displayName) });
    }
  }
  return { models: [...models.values()], default_model: defaultModel };
}

export class ClaudeAuthenticationError extends Error {
  code = "authentication_required";
}

export function isClaudeAccount(account) {
  return account?.loggedIn === true && account.authMethod === "claude.ai"
    && account.apiProvider === "firstParty" && typeof account.subscriptionType === "string"
    && /^[a-z][a-z0-9_-]*$/.test(account.subscriptionType);
}

export function requireClaudeAccount(account) {
  if (!isClaudeAccount(account)) {
    throw new ClaudeAuthenticationError("Reconnect Claude to load the subscription account and its runtime default");
  }
}

export function guardSdkMessage(message) {
  if (message.type === "system" && message.subtype === "model_refusal_fallback") {
    throw new Error("Claude attempted an automatic model switch. Request stopped; choose a model explicitly in Settings.");
  }
  const errors = [message.error, ...(message.errors ?? [])].filter(error => typeof error === "string");
  if (errors.some(error => /authentication_failed|not logged in|login expired/i.test(error))) {
    throw new ClaudeAuthenticationError("Claude sign-in expired. Please reconnect Claude.");
  }
}

export function redactError(error) {
  return (error instanceof Error ? error.message : String(error))
    .replace(/sk-ant-[A-Za-z0-9_-]+/g, "[redacted]")
    .replace(/Bearer\s+[^\s"']+/gi, "Bearer [redacted]")
    .slice(0, 1600);
}
