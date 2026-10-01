import { describe, expect, test } from "bun:test";
import { z } from "zod";
import { bootstrapObserver, buildToolServer, createToolBridge, guardSdkMessage, isClaudeAccount, isolatedEnvironment, nativeQueryOptions, normalizeCatalog, queryOptions, redactError, zodShapeFromJsonSchema } from "./protocol.mjs";

const VOLUME_SCHEMA = { type: "object", properties: { level: { type: "integer", minimum: 0, maximum: 100, description: "Target volume" } }, required: ["level"], additionalProperties: false };
const PROMPT_SCHEMA = { type: "object", properties: { type: { type: "string", enum: ["insight", "chat"] }, content: { type: "string" }, note: { type: "string" } }, required: ["type", "content"], additionalProperties: false };

describe("native tool bridge", () => {
  test("registry JSON Schema converts to Zod with the same constraints", () => {
    const volume = z.object(zodShapeFromJsonSchema(VOLUME_SCHEMA));
    expect(volume.safeParse({ level: 30 }).success).toBe(true);
    expect(volume.safeParse({ level: "30" }).success).toBe(false);
    expect(volume.safeParse({ level: 120 }).success).toBe(false);
    expect(volume.safeParse({ level: 30.5 }).success).toBe(false);
    const prompt = z.object(zodShapeFromJsonSchema(PROMPT_SCHEMA));
    expect(prompt.safeParse({ type: "chat", content: "x" }).success).toBe(true);
    expect(prompt.safeParse({ type: "lyrics", content: "x" }).success).toBe(false);
    expect(prompt.safeParse({ type: "chat" }).success).toBe(false);
    expect(() => zodShapeFromJsonSchema({ type: "object", properties: { x: { type: "array" } } })).toThrow("unsupported");
  });

  test("tool server only relays calls; results map ok -> isError", async () => {
    const seen = [];
    const server = buildToolServer(
      [{ name: "set_volume", description: "Set volume", parameters: VOLUME_SCHEMA }],
      async (call) => { seen.push(call); return { ok: call.args.level < 50, output: `level ${call.args.level}` }; },
    );
    expect(server.type).toBe("sdk");
    expect(server.name).toBe("expotify");
    const options = nativeQueryOptions({ configDir: "/tmp/x", maxTurns: 3 }, "/cli", { HOME: "/h" }, server);
    expect(options.maxTurns).toBe(3);
    expect(options.allowedTools).toEqual(["mcp__expotify__*"]);
    expect(options.env.ENABLE_TOOL_SEARCH).toBe("false");
    expect(options.tools).toEqual([]);
    expect(nativeQueryOptions({ configDir: "/tmp/x" }, "/cli", {}, server).maxTurns).toBe(4);
  });

  test("bridge resolves calls by id and fails pending calls when Rust goes away", async () => {
    const lines = [];
    const bridge = createToolBridge((line) => lines.push(line));
    const first = bridge.call({ name: "set_volume", args: { level: 30 } });
    const second = bridge.call({ name: "like_current", args: {} });
    expect(lines.map((l) => JSON.parse(l))).toEqual([
      { type: "tool_call", id: "call-1", name: "set_volume", args: { level: 30 } },
      { type: "tool_call", id: "call-2", name: "like_current", args: {} },
    ]);
    expect(bridge.feed("not json")).toBe(false);
    expect(bridge.feed(JSON.stringify({ type: "tool_result", id: "call-9", ok: true, output: "x" }))).toBe(false);
    expect(bridge.feed(JSON.stringify({ type: "tool_result", id: "call-2", ok: false, output: "Nothing is playing" }))).toBe(true);
    await expect(second).resolves.toEqual({ ok: false, output: "Nothing is playing" });
    bridge.close("stdin closed");
    await expect(first).rejects.toThrow("stdin closed");
    await expect(bridge.call({ name: "x", args: {} })).rejects.toThrow("stdin closed");
    expect(bridge.pendingCount).toBe(0);
  });
});

const subscription = { loggedIn: true, authMethod: "claude.ai", apiProvider: "firstParty", subscriptionType: "max" };

describe("Claude protocol", () => {
  test("isolates credentials, settings and provider overrides", () => {
    const env = isolatedEnvironment("/tmp/expotify-test", {
      HOME: "/Users/test", HTTPS_PROXY: "http://localhost:8080",
      ANTHROPIC_API_KEY: "secret", ANTHROPIC_MODEL: "override",
      CLAUDE_CODE_OAUTH_TOKEN: "secret", CLAUDE_CONFIG_DIR: "/other",
      NODE_OPTIONS: "--require bad.js", PATH: "/external",
    });
    expect(env.ANTHROPIC_API_KEY).toBeUndefined();
    expect(env.ANTHROPIC_MODEL).toBeUndefined();
    expect(env.CLAUDE_CODE_OAUTH_TOKEN).toBeUndefined();
    expect(env.NODE_OPTIONS).toBeUndefined();
    expect(env.CLAUDE_CONFIG_DIR).toBe("/tmp/expotify-test");
    expect(env.HOME).toBe("/Users/test");
    expect(env.PATH).toBe("/usr/bin:/bin:/usr/sbin:/sbin");
  });

  test("default is omitted from SDK options; concrete choices stay literal", () => {
    const request = { configDir: "/tmp/test", prompt: "hello" };
    expect(queryOptions(request, "/cli", {})).not.toHaveProperty("model");
    expect(queryOptions({ ...request, model: "claude-test-5" }, "/cli", {}).model).toBe("claude-test-5");
    expect(queryOptions(request, "/cli", {}).settingSources).toEqual([]);
  });

  test("resolves aliases and retains the default as a concrete option", () => {
    const result = normalizeCatalog([
      { value: "default", resolvedModel: "claude-opus-5" },
      { value: "opus", resolvedModel: "claude-opus-5" },
      { value: "fable", resolvedModel: "claude-fable-5-1[1m]" },
      { value: "haiku", resolvedModel: "claude-haiku-4-5-20251001" },
    ], subscription);
    expect(result.default_model).toBe("claude-opus-5");
    expect(result.models).toEqual([
      { id: "claude-opus-5", name: "Opus 5" },
      { id: "claude-fable-5-1[1m]", name: "Fable 5.1 (1M)" },
      { id: "claude-haiku-4-5-20251001", name: "Haiku 4.5" },
    ]);
  });

  test("never invents a default when native account metadata is missing", () => {
    expect(() => normalizeCatalog([], {})).toThrow("Reconnect Claude");
    expect(() => normalizeCatalog([], subscription)).toThrow("resolved default");
  });

  test("errors never expose bearer credentials", () => {
    expect(redactError(new Error("sk-ant-secret Bearer token-value"))).toBe("[redacted] Bearer [redacted]");
  });

  test("unchanged successful bootstrap is fresh without a rewritten timestamp", () => {
    const observer = bootstrapObserver();
    expect(observer.isFresh(100, 200)).toBe(false);
    observer.observe("[Bootstrap] Cache unchanged, skip");
    observer.observe("ping write\n");
    expect(observer.isFresh(100, 200)).toBe(true);
    expect(observer.isFresh(0, 200)).toBe(false);
    expect(bootstrapObserver().isFresh(210, 200)).toBe(true);
  });

  test("auth requires native subscription metadata, not SDK display labels or global API profiles", () => {
    expect(isClaudeAccount(subscription)).toBe(true);
    expect(isClaudeAccount({ ...subscription, subscriptionType: "Claude API" })).toBe(false);
    expect(isClaudeAccount({ ...subscription, subscriptionType: null })).toBe(false);
    expect(isClaudeAccount({ ...subscription, authMethod: "api_key" })).toBe(false);
    expect(isClaudeAccount({ ...subscription, authMethod: "oauth_token" })).toBe(false);
    expect(isClaudeAccount({ ...subscription, apiProvider: "gateway" })).toBe(false);
  });

  test("automatic model substitution is surfaced as an error", () => {
    expect(() => guardSdkMessage({ type: "system", subtype: "model_refusal_fallback" })).toThrow("automatic model switch");
    expect(() => guardSdkMessage({ type: "assistant", error: "authentication_failed" })).toThrow("reconnect Claude");
    expect(() => guardSdkMessage({ type: "result", errors: ["Not logged in"] })).toThrow("reconnect Claude");
    expect(() => guardSdkMessage({ type: "assistant", message: { content: [] } })).not.toThrow();
  });
});
