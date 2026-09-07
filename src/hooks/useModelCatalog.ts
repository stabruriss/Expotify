import { useCallback, useEffect, useRef, useState } from "react";
import { listModels } from "../lib/tauri";
import type { AuthStatus, ProviderCatalog } from "../types";

export function useModelCatalog(auth: AuthStatus, visible: boolean) {
  const [catalogs, setCatalogs] = useState<ProviderCatalog[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = useCallback(async (force = true) => {
    const current = ++generation.current;
    if (!auth.openai && !auth.anthropic) {
      setCatalogs([]); setLoading(false); setError(null); return;
    }
    setLoading(true); setError(null);
    try {
      const result = await listModels(force);
      if (current === generation.current) setCatalogs(result);
    } catch (error) {
      if (current === generation.current) setError(String(error));
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }, [auth.openai, auth.anthropic]);

  useEffect(() => {
    setCatalogs(previous => previous.filter(catalog => auth[catalog.provider]));
    void refresh(false);
    return () => { generation.current++; };
  }, [auth.openai, auth.anthropic, refresh]);

  useEffect(() => { if (visible) void refresh(false); }, [visible, refresh]);

  return { catalogs: catalogs.filter(catalog => auth[catalog.provider]), loading, error, refresh };
}
