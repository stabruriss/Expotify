import { useCallback, useRef, useState } from "react";
import { ttsCheckAvailable } from "../lib/tauri";

export function useSpeechService() {
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [available, setAvailable] = useState(false);
  const pendingCheck = useRef<Promise<boolean> | null>(null);

  const reportError = useCallback((cause: unknown) => {
    const detail = cause instanceof Error ? cause.message : String(cause);
    setError(`Read-aloud failed: ${detail}`);
  }, []);

  const checkAvailability = useCallback((): Promise<boolean> => {
    if (pendingCheck.current) return pendingCheck.current;
    setChecking(true);
    setError(null);
    const check = (async () => {
      try {
        await ttsCheckAvailable();
        setAvailable(true);
        return true;
      } catch (cause) {
        setAvailable(false);
        reportError(cause);
        return false;
      } finally {
        pendingCheck.current = null;
        setChecking(false);
      }
    })();
    pendingCheck.current = check;
    return check;
  }, [reportError]);

  const dismissError = useCallback(() => setError(null), []);
  return { error, checking, available, checkAvailability, reportError, dismissError };
}
