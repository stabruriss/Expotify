import { useCallback, useEffect, useRef, useState } from "react";
import { listModels } from "../lib/tauri";
import type { AuthStatus, ProviderCatalog } from "../types";

export function useModelCatalog(auth: AuthStatus, visible: boolean, discoverMagpie: () => Promise<boolean>) {
  const [catalogs, setCatalogs] = useState<ProviderCatalog[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = useCallback(async (force = true, discover = true) => {
    const current = ++generation.current;
    setLoading(true); setError(null);
    try {
      if (discover) await discoverMagpie().catch(() => false);
      const result = await listModels(force);
      if (current === generation.current) setCatalogs(result);
    } catch (error) {
      if (current === generation.current) setError(String(error));
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }, [discoverMagpie]);

  useEffect(() => {
    setCatalogs(previous => previous.filter(catalog => catalog.provider === "magpie" || auth[catalog.provider]));
    void refresh(false, false);
    return () => { generation.current++; };
  }, [auth.openai, auth.anthropic, auth.magpie, refresh]);

  useEffect(() => { if (visible) void refresh(false); }, [visible, refresh]);

  return { catalogs: catalogs.filter(catalog => catalog.provider === "magpie" || auth[catalog.provider]), loading, error, refresh };
}
