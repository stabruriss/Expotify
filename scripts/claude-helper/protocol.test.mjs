import { describe, expect, test } from "bun:test";
import { bootstrapObserver, guardSdkMessage, isClaudeAccount, isolatedEnvironment, normalizeCatalog, queryOptions, redactError } from "./protocol.mjs";

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
