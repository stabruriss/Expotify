import type { AuthStatus, ModelProvider, ModelSelection, ProviderCatalog } from "../types";

export const PROVIDERS: ModelProvider[] = ["openai", "anthropic", "magpie"];
const PROVIDER_LABELS: Record<ModelProvider, string> = { openai: "ChatGPT", anthropic: "Claude", magpie: "Magpie" };
export const providerLabel = (provider: ModelProvider) => PROVIDER_LABELS[provider];
export const hasAiProvider = (connected: Pick<AuthStatus, ModelProvider>) => PROVIDERS.some(provider => connected[provider]);
export const selectionKey = (selection: ModelSelection) =>
  JSON.stringify(selection.mode === "default" ? [selection.provider, "default"] : [selection.provider, "fixed", selection.model]);
export const sameSelection = (a: ModelSelection, b: ModelSelection) => selectionKey(a) === selectionKey(b);

export interface ModelOption {
  selection: ModelSelection;
  name: string;
  detail?: string;
  searchTerms?: string[];
  disabled: boolean;
}

export function modelOptions(catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): ModelOption[] {
  const providers = PROVIDERS.filter(provider => connected[provider]);
  const defaults: ModelOption[] = providers.flatMap(provider => {
    const catalog = catalogs.find(catalog => catalog.provider === provider);
    const current = catalog?.models.find(model => model.id === catalog.default_model);
    if (provider === "magpie" && !current) return [];
    return [{
      selection: { provider, mode: "default" as const }, name: "Default",
      detail: current ? current.name : "Default unavailable",
      searchTerms: current ? [current.id] : [],
      disabled: !current,
    }];
  });
  const fixed: ModelOption[] = providers.flatMap(provider =>
    (catalogs.find(catalog => catalog.provider === provider)?.models ?? []).map(model => ({
      selection: { provider, mode: "fixed" as const, model: model.id },
      name: provider === "magpie" ? magpieModelLabel(model.id, model.name) : model.name,
      searchTerms: [model.name], disabled: false,
    })),
  );
  return [...defaults, ...fixed];
}

function magpieModelLabel(id: string, name = id): string {
  const slash = id.indexOf("/");
  const label = name !== id ? name : id.slice(slash + 1);
  return slash > 0 && slash < id.length - 1 ? `${id.slice(0, slash)} / ${label}` : name;
}

const normalizeSearch = (value: string) => value.normalize("NFKC").toLowerCase();

export function filterModelOptions(options: ModelOption[], query: string): ModelOption[] {
  const terms = normalizeSearch(query).trim().split(/\s+/).filter(Boolean);
  if (!terms.length) return options;
  return options.filter(option => {
    const text = normalizeSearch([
      providerLabel(option.selection.provider), option.selection.provider, option.name, option.detail ?? "",
      option.selection.mode === "fixed" ? option.selection.model : "default",
      ...(option.searchTerms ?? []),
    ].join(" "));
    return terms.every(term => text.includes(term));
  });
}

export function describeSelection(selection: ModelSelection, catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): ModelOption {
  const option = modelOptions(catalogs, connected).find(option => sameSelection(option.selection, selection));
  if (option) return option;
  return { selection, name: selection.mode === "default" ? "Default"
    : selection.provider === "magpie" ? magpieModelLabel(selection.model) : selection.model,
    detail: connected[selection.provider] ? "Not in the current model list"
      : selection.provider === "magpie" ? "Gateway unavailable" : "Account disconnected", disabled: true };
}

export function selectionTitle(selection: ModelSelection, catalogs: ProviderCatalog[]): string {
  const catalog = catalogs.find(item => item.provider === selection.provider);
  const modelId = selection.mode === "fixed" ? selection.model : catalog?.default_model;
  const model = catalog?.models.find(item => item.id === modelId);
  const name = model?.name.trim() ? model.name : modelId || "Default unavailable";
  const title = `${providerLabel(selection.provider)}: ${selection.mode === "default" ? "Default - " : ""}${name}`;
  return modelId && modelId !== name ? `${title}\nID: ${modelId}` : title;
}

export function canSaveSelection(next: ModelSelection, saved: ModelSelection, catalogs: ProviderCatalog[], connected: Pick<AuthStatus, ModelProvider>): boolean {
  // Editing an unrelated setting must not force migration of an unavailable model.
  return sameSelection(next, saved) || !describeSelection(next, catalogs, connected).disabled;
}
