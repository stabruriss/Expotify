import { useState, useEffect, useCallback } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { AuthStatus } from "../types";
import {
  getAuthStatus,
  spotifyGetAuthUrl,
  spotifyExchangeCode,
  spotifyLogout,
  openaiGetAuthUrl,
  openaiExchangeCode,
  openaiLogout,
} from "../lib/tauri";

export function useAuth() {
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    spotify: false,
    openai: false,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Check auth status on mount
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

  // Spotify login
  const loginSpotify = useCallback(async () => {
    try {
      setError(null);
      const authUrl = await spotifyGetAuthUrl();
      await openUrl(authUrl);
      // The callback will be handled by a local server
      // For now, we'll need to implement callback handling
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // Handle Spotify callback
  const handleSpotifyCallback = useCallback(async (code: string) => {
    try {
      setError(null);
      await spotifyExchangeCode(code);
      setAuthStatus((prev) => ({ ...prev, spotify: true }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // Spotify logout
  const logoutSpotify = useCallback(async () => {
    try {
      setError(null);
      await spotifyLogout();
      setAuthStatus((prev) => ({ ...prev, spotify: false }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // OpenAI login
  const loginOpenai = useCallback(async () => {
    try {
      setError(null);
      const authUrl = await openaiGetAuthUrl();
      await openUrl(authUrl);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // Handle OpenAI callback
  const handleOpenaiCallback = useCallback(
    async (code: string, state: string) => {
      try {
        setError(null);
        await openaiExchangeCode(code, state);
        setAuthStatus((prev) => ({ ...prev, openai: true }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    []
  );

  // OpenAI logout
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
    loginSpotify,
    handleSpotifyCallback,
    logoutSpotify,
    loginOpenai,
    handleOpenaiCallback,
    logoutOpenai,
  };
}
