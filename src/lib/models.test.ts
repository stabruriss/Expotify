import { describe, expect, test } from "bun:test";
import { canSaveSelection, describeSelection, modelOptions, sameSelection } from "./models";
import type { ModelSelection, ProviderCatalog } from "../types";

const catalogs: ProviderCatalog[] = ["openai", "anthropic"].map((provider, i) => ({
  provider: provider as ProviderCatalog["provider"], default_model: `model-${i}`, stale: false, fetched_at: "2026-09-06T00:00:00Z", error: null,
  models: [{ id: `model-${i}`, name: `Model ${i}` }, { id: `extra-${i}`, name: `Extra ${i}` }],
}));

describe("model selection semantics", () => {
  test("connected accounts determine zero, one or two defaults", () => {
    expect(modelOptions(catalogs, { openai: false, anthropic: false })).toEqual([]);
    const one = modelOptions(catalogs, { openai: false, anthropic: true });
    expect(one.filter(row => row.selection.mode === "default")).toHaveLength(1);
    expect(one.every(row => row.selection.provider === "anthropic")).toBe(true);
    const both = modelOptions(catalogs, { openai: true, anthropic: true });
    expect(both.slice(0, 2).every(row => row.selection.mode === "default")).toBe(true);
    expect(both).toHaveLength(6);
  });
  test("default and its concrete target are distinct choices", () => {
    const dynamic: ModelSelection = { provider: "openai", mode: "default" };
    const fixed: ModelSelection = { provider: "openai", mode: "fixed", model: "model-0" };
    expect(sameSelection(dynamic, fixed)).toBe(false);
    const updated = [{ ...catalogs[0], default_model: "extra-0" }];
    expect(describeSelection(dynamic, updated, { openai: true, anthropic: false }).detail).toBe("Extra 0");
    expect(describeSelection(fixed, updated, { openai: true, anthropic: false }).name).toBe("Model 0");
  });
  test("missing models and disconnected accounts are never silently replaced", () => {
    const old: ModelSelection = { provider: "openai", mode: "fixed", model: "retired" };
    const auth = { openai: false, anthropic: true };
    expect(describeSelection(old, catalogs, auth)).toMatchObject({ name: "retired", disabled: true });
    expect(canSaveSelection(old, old, catalogs, auth)).toBe(true);
    expect(canSaveSelection({ provider: "openai", mode: "default" }, old, catalogs, auth)).toBe(false);
  });
  test("catalog failure never fabricates models or defaults", () => {
    const result = modelOptions([], { openai: true, anthropic: true });
    expect(result).toHaveLength(2);
    expect(result.every(row => row.disabled)).toBe(true);
  });
});
