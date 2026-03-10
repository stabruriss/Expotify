import { useState, useEffect, useCallback } from "react";
import type { AuthStatus } from "../types";
import {
  anthropicCancelOAuth,
  anthropicCompleteOAuth,
  anthropicLogout,
  anthropicStartOAuth,
  getAuthStatus,
  openaiLogin,
  openaiLogout,
  spotifyConnect,
  spotifyDisconnect,
  spotifyLogin,
} from "../lib/tauri";

export function useAuth() {
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    openai: false,
    anthropic: false,
    anthropic_available: false,
    spotify: false,
  });
  const [loading, setLoading] = useState(true);
  const [spotifyLoading, setSpotifyLoading] = useState(false);
  const [anthropicPending, setAnthropicPending] = useState(false);
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

  const loginSpotify = useCallback(async () => {
    try {
      setError(null);
      setSpotifyLoading(true);
      await spotifyLogin();
      setAuthStatus((prev) => ({ ...prev, spotify: true }));
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      if (!msg.includes("cancelled")) {
        setError(msg);
      }
    } finally {
      setSpotifyLoading(false);
    }
  }, []);

  const connectSpotify = useCallback(async (spDc: string) => {
    try {
      setError(null);
      setSpotifyLoading(true);
      await spotifyConnect(spDc);
      setAuthStatus((prev) => ({ ...prev, spotify: true }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      throw err;
    } finally {
      setSpotifyLoading(false);
    }
  }, []);

  const disconnectSpotify = useCallback(async () => {
    try {
      setError(null);
      await spotifyDisconnect();
      setAuthStatus((prev) => ({ ...prev, spotify: false }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const startAnthropicLogin = useCallback(async () => {
    try {
      setError(null);
      setLoading(true);
      await anthropicStartOAuth();
      setAnthropicPending(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setAnthropicPending(false);
    } finally {
      setLoading(false);
    }
  }, []);

  const completeAnthropicLogin = useCallback(async (code: string) => {
    const trimmed = code.trim();
    if (!trimmed) {
      const message = "Authorization code is required";
      setError(message);
      throw new Error(message);
    }

    try {
      setError(null);
      setLoading(true);
      await anthropicCompleteOAuth(trimmed);
      setAuthStatus((prev) => ({ ...prev, anthropic: true, anthropic_available: true }));
      setAnthropicPending(false);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const cancelAnthropicLogin = useCallback(async () => {
    try {
      setError(null);
      await anthropicCancelOAuth();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setAnthropicPending(false);
    }
  }, []);

  const logoutAnthropic = useCallback(async () => {
    try {
      setError(null);
      await anthropicLogout();
      setAnthropicPending(false);
      setAuthStatus((prev) => ({ ...prev, anthropic: false }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  return {
    authStatus,
    loading,
    spotifyLoading,
    anthropicPending,
    error,
    checkAuthStatus,
    loginOpenai,
    logoutOpenai,
    startAnthropicLogin,
    completeAnthropicLogin,
    cancelAnthropicLogin,
    logoutAnthropic,
    loginSpotify,
    connectSpotify,
    disconnectSpotify,
  };
}
