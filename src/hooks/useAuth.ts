import { useState, useEffect, useCallback, useRef } from "react";
import type { AuthStatus } from "../types";
import {
  anthropicCancelOAuth,
  anthropicLogout,
  anthropicStartOAuth,
  getAuthStatus,
  openaiLogin,
  openaiLogout,
  probeMagpie,
  spotifyConnect,
  spotifyDisconnect,
  spotifyLogin,
} from "../lib/tauri";

export function useAuth() {
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    openai: false,
    anthropic: false,
    magpie: false,
    anthropic_available: false,
    spotify: false,
  });
  const [loading, setLoading] = useState(true);
  const [spotifyLoading, setSpotifyLoading] = useState(false);
  const [anthropicPending, setAnthropicPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const magpieRevision = useRef(0);
  const pendingMagpie = useRef<Promise<boolean> | null>(null);

  useEffect(() => {
    checkAuthStatus();
  }, []);

  const checkAuthStatus = useCallback(async () => {
    const revision = magpieRevision.current;
    try {
      setLoading(true);
      const status = await getAuthStatus();
      setAuthStatus(previous => ({ ...status,
        magpie: revision === magpieRevision.current ? status.magpie : previous.magpie,
      }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const discoverMagpie = useCallback(() => {
    if (pendingMagpie.current) return pendingMagpie.current;
    const publish = (available: boolean) => {
      magpieRevision.current++;
      setAuthStatus(previous => ({ ...previous, magpie: available }));
      return available;
    };
    const request = probeMagpie().then(publish, error => {
      publish(false);
      throw error;
    }).finally(() => { pendingMagpie.current = null; });
    pendingMagpie.current = request;
    return request;
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
      setAnthropicPending(true);
      await anthropicStartOAuth();
      setAuthStatus((prev) => ({ ...prev, anthropic: true, anthropic_available: true }));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (!message.includes("cancelled")) setError(message);
    } finally {
      setAnthropicPending(false);
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
    discoverMagpie,
    loginOpenai,
    logoutOpenai,
    startAnthropicLogin,
    cancelAnthropicLogin,
    logoutAnthropic,
    loginSpotify,
    connectSpotify,
    disconnectSpotify,
  };
}
