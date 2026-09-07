import type { AuthStatus, ModelProvider, ModelSelection, ProviderCatalog } from "../types";

export const PROVIDERS: ModelProvider[] = ["openai", "anthropic"];
export const providerLabel = (provider: ModelProvider) => provider === "openai" ? "ChatGPT" : "Claude";
export const selectionKey = (selection: ModelSelection) =>
  JSON.stringify(selection.mode === "default" ? [selection.provider, "default"] : [selection.provider, "fixed", selection.model]);
export const sameSelection = (a: ModelSelection, b: ModelSelection) => selectionKey(a) === selectionKey(b);

export interface ModelOption {
  selection: ModelSelection;
  name: string;
  detail?: string;
  disabled: boolean;
}

export function modelOptions(catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): ModelOption[] {
  const providers = PROVIDERS.filter(provider => connected[provider]);
  const defaults: ModelOption[] = providers.map(provider => {
    const catalog = catalogs.find(catalog => catalog.provider === provider);
    const current = catalog?.models.find(model => model.id === catalog.default_model);
    return {
      selection: { provider, mode: "default" }, name: "Default",
      detail: current ? current.name : "Default unavailable",
      disabled: !current,
    };
  });
  const fixed: ModelOption[] = providers.flatMap(provider =>
    (catalogs.find(catalog => catalog.provider === provider)?.models ?? []).map(model => ({
      selection: { provider, mode: "fixed" as const, model: model.id }, name: model.name, disabled: false,
    })),
  );
  return [...defaults, ...fixed];
}

export function describeSelection(selection: ModelSelection, catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): ModelOption {
  const option = modelOptions(catalogs, connected).find(option => sameSelection(option.selection, selection));
  if (option) return option;
  return { selection, name: selection.mode === "default" ? "Default" : selection.model,
    detail: connected[selection.provider] ? "Not in the current model list" : "Account disconnected", disabled: true };
}

export function canSaveSelection(next: ModelSelection, saved: ModelSelection, catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): boolean {
  // Editing an unrelated setting must not force migration of an unavailable model.
  return sameSelection(next, saved) || !describeSelection(next, catalogs, connected).disabled;
}
