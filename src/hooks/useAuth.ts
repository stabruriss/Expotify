import { useState, useEffect, useCallback } from "react";
import type { AuthStatus } from "../types";
import { getAuthStatus, openaiLogin, openaiLogout } from "../lib/tauri";

export function useAuth() {
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    openai: false,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    checkAuthStatus();
  }, []);

  const checkAuthStatus = useCallback(async () => {
    try {
      setLoading(true);
      const status = await getAuthStatus();
      setAuthStatus(status);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const loginOpenai = useCallback(async () => {
    try {
      setError(null);
      setLoading(true);
      await openaiLogin();
      setAuthStatus((prev) => ({ ...prev, openai: true }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const logoutOpenai = useCallback(async () => {
    try {
      setError(null);
      await openaiLogout();
      setAuthStatus((prev) => ({ ...prev, openai: false }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  return {
    authStatus,
    loading,
    error,
    checkAuthStatus,
    loginOpenai,
    logoutOpenai,
  };
}
