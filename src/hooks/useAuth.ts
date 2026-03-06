import { useState, useEffect, useCallback } from "react";
import type { AuthStatus } from "../types";
import { getAuthStatus, openaiLogin, openaiLogout, spotifyConnect, spotifyLogin, spotifyDisconnect, anthropicActivate, anthropicDeactivate } from "../lib/tauri";

export function useAuth() {
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    openai: false,
    anthropic: false,
    anthropic_available: false,
    spotify: false,
  });
  const [loading, setLoading] = useState(true);
  const [spotifyLoading, setSpotifyLoading] = useState(false);
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
      // Don't show error for user cancellation
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

  const activateAnthropic = useCallback(async () => {
    try {
      setError(null);
      setLoading(true);
      await anthropicActivate();
      setAuthStatus((prev) => ({ ...prev, anthropic: true }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const deactivateAnthropic = useCallback(async () => {
    try {
      setError(null);
      await anthropicDeactivate();
      setAuthStatus((prev) => ({ ...prev, anthropic: false }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  return {
    authStatus,
    loading,
    spotifyLoading,
    error,
    checkAuthStatus,
    loginOpenai,
    logoutOpenai,
    activateAnthropic,
    deactivateAnthropic,
    loginSpotify,
    connectSpotify,
    disconnectSpotify,
  };
}
