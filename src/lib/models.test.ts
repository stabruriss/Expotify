import { describe, expect, test } from "bun:test";
import { canSaveSelection, describeSelection, filterModelOptions, hasAiProvider, modelOptions, sameSelection, selectionTitle } from "./models";
import type { ModelSelection, ProviderCatalog } from "../types";

const catalogs: ProviderCatalog[] = ["openai", "anthropic"].map((provider, i) => ({
  provider: provider as ProviderCatalog["provider"], default_model: `model-${i}`, stale: false, fetched_at: "2026-09-06T00:00:00Z", error: null,
  models: [{ id: `model-${i}`, name: `Model ${i}` }, { id: `extra-${i}`, name: `Extra ${i}` }],
}));

describe("model selection semantics", () => {
  test("connected accounts determine zero, one or two defaults", () => {
    expect(modelOptions(catalogs, { openai: false, anthropic: false, magpie: false })).toEqual([]);
    const one = modelOptions(catalogs, { openai: false, anthropic: true, magpie: false });
    expect(one.filter(row => row.selection.mode === "default")).toHaveLength(1);
    expect(one.every(row => row.selection.provider === "anthropic")).toBe(true);
    const both = modelOptions(catalogs, { openai: true, anthropic: true, magpie: false });
    expect(both.slice(0, 2).every(row => row.selection.mode === "default")).toBe(true);
    expect(both).toHaveLength(6);
  });
  test("default and its concrete target are distinct choices", () => {
    const dynamic: ModelSelection = { provider: "openai", mode: "default" };
    const fixed: ModelSelection = { provider: "openai", mode: "fixed", model: "model-0" };
    expect(sameSelection(dynamic, fixed)).toBe(false);
    const updated = [{ ...catalogs[0], default_model: "extra-0" }];
    expect(describeSelection(dynamic, updated, { openai: true, anthropic: false, magpie: false }).detail).toBe("Extra 0");
    expect(describeSelection(fixed, updated, { openai: true, anthropic: false, magpie: false }).name).toBe("Model 0");
  });
  test("missing models and disconnected accounts are never silently replaced", () => {
    const old: ModelSelection = { provider: "openai", mode: "fixed", model: "retired" };
    const auth = { openai: false, anthropic: true, magpie: false };
    expect(describeSelection(old, catalogs, auth)).toMatchObject({ name: "retired", disabled: true });
    expect(canSaveSelection(old, old, catalogs, auth)).toBe(true);
    expect(canSaveSelection({ provider: "openai", mode: "default" }, old, catalogs, auth)).toBe(false);
  });
  test("catalog failure never fabricates models or defaults", () => {
    const result = modelOptions([], { openai: true, anthropic: true, magpie: false });
    expect(result).toHaveLength(2);
    expect(result.every(row => row.disabled)).toBe(true);
  });
});

describe("model keyword search", () => {
  const options = modelOptions(catalogs, { openai: true, anthropic: true, magpie: false });

  test("empty queries preserve the original provider and default ordering", () => {
    expect(filterModelOptions(options, " \n ")).toBe(options);
  });

  test("matches all terms across provider labels and model names without case sensitivity", () => {
    const matches = filterModelOptions(options, "  cHaTgPt   ExTrA  ");
    expect(matches).toHaveLength(1);
    expect(matches[0].selection).toEqual({ provider: "openai", mode: "fixed", model: "extra-0" });
    expect(filterModelOptions(options, "claude extra-0")).toEqual([]);
    expect(filterModelOptions(options, "anthropic extra")[0].selection.provider).toBe("anthropic");
  });

  test("matches full-width queries and concrete IDs, including the default target", () => {
    const matches = filterModelOptions(options, "ＭＯＤＥＬ-０");
    expect(matches.map(option => option.selection.mode)).toEqual(["default", "fixed"]);
    expect(matches.every(option => option.selection.provider === "openai")).toBe(true);
  });

  test("treats punctuation literally and returns an empty list for unknown keywords", () => {
    expect(filterModelOptions(options, "[.*]")).toEqual([]);
    expect(filterModelOptions(options, "no-such-model")).toEqual([]);
  });
});

describe("Magpie model selection", () => {
  const auth = { openai: false, anthropic: false, magpie: true };
  const gateway: ProviderCatalog = {
    provider: "magpie", default_model: null, stale: false, error: null, fetched_at: null,
    models: [
      { id: "alpha/shared-model", name: "Shared Model" },
      { id: "beta/shared-model", name: "Shared Model" },
      { id: "router/vendor/nested-model", name: "router/vendor/nested-model" },
    ],
  };

  test("a gateway alone enables AI and offers only concrete models, never a fabricated default", () => {
    expect(hasAiProvider(auth)).toBe(true);
    expect(hasAiProvider({ ...auth, magpie: false })).toBe(false);
    const options = modelOptions([gateway], auth);
    expect(options).toHaveLength(3);
    expect(options.every(option => option.selection.mode === "fixed")).toBe(true);
    expect(modelOptions([], auth)).toEqual([]);
  });

  test("upstreams remain distinct and only the first slash is split for display", () => {
    const options = modelOptions([gateway], auth);
    expect(options.map(option => option.name)).toEqual(["alpha / Shared Model", "beta / Shared Model", "router / vendor/nested-model"]);
    expect(options.map(option => option.selection)).toEqual(gateway.models.map(model => ({ provider: "magpie", mode: "fixed", model: model.id })));
    expect(filterModelOptions(options, "magpie beta shared").map(option => option.name)).toEqual(["beta / Shared Model"]);
    expect(filterModelOptions(options, "Nested Model")).toHaveLength(1);
  });

  test("an offline selected gateway stays selected and unrelated settings remain saveable", () => {
    const saved: ModelSelection = { provider: "magpie", mode: "fixed", model: "beta/shared-model" };
    const offline = { ...auth, magpie: false };
    expect(modelOptions([gateway], offline)).toEqual([]);
    expect(describeSelection(saved, [gateway], offline)).toMatchObject({ selection: saved, name: "beta / shared-model", detail: "Gateway unavailable", disabled: true });
    expect(canSaveSelection(saved, saved, [gateway], offline)).toBe(true);
    expect(canSaveSelection({ ...saved, model: "alpha/shared-model" }, saved, [gateway], offline)).toBe(false);
  });
});

describe("model hover titles", () => {
  const id = "custom-vllm/meta-llama/Llama-3.1-405B-Instruct-FP8-dynamic";
  const name = "Llama 3.1 405B Instruct - Extended Context - Evaluation Build A";
  const selection: ModelSelection = { provider: "magpie", mode: "fixed", model: id };
  const gateway: ProviderCatalog = {
    provider: "magpie", default_model: null, stale: false, error: null, fetched_at: null,
    models: [{ id, name }],
  };

  test("preserves the complete display name and raw id independently of ellipsis", () => {
    expect(selectionTitle(selection, [gateway])).toBe(`Magpie: ${name}\nID: ${id}`);
    const options = modelOptions([gateway], { openai: false, anthropic: false, magpie: true });
    expect(filterModelOptions(options, "FP8-dynamic Evaluation")).toHaveLength(1);
    expect(options[0].selection).toEqual(selection);
  });

  test("missing or whitespace-only names fall back to the entire id without duplicating it", () => {
    expect(selectionTitle(selection, [])).toBe(`Magpie: ${id}`);
    for (const name of [id, "", "   "]) {
      expect(selectionTitle(selection, [{ ...gateway, models: [{ id, name }] }])).toBe(`Magpie: ${id}`);
    }
  });

  test("default titles reflect the catalog target without turning the selection into a fixed model", () => {
    const dynamic: ModelSelection = { provider: "openai", mode: "default" };
    expect(selectionTitle(dynamic, catalogs)).toBe("ChatGPT: Default - Model 0\nID: model-0");
    expect(selectionTitle(dynamic, [{ ...catalogs[0], default_model: "extra-0" }]))
      .toBe("ChatGPT: Default - Extra 0\nID: extra-0");
    expect(selectionTitle(dynamic, [])).toBe("ChatGPT: Default - Default unavailable");
  });
});
