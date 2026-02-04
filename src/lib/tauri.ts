import { invoke } from "@tauri-apps/api/core";
import type { TrackInfo, Settings, AuthStatus } from "../types";

// ============ Spotify Auth ============

export async function spotifyIsAuthenticated(): Promise<boolean> {
  return invoke("spotify_is_authenticated");
}

export async function spotifyGetAuthUrl(): Promise<string> {
  return invoke("spotify_get_auth_url");
}

export async function spotifyExchangeCode(code: string): Promise<void> {
  return invoke("spotify_exchange_code", { code });
}

export async function spotifyLogout(): Promise<void> {
  return invoke("spotify_logout");
}

// ============ OpenAI Auth ============

export async function openaiIsAuthenticated(): Promise<boolean> {
  return invoke("openai_is_authenticated");
}

export async function openaiGetAuthUrl(): Promise<string> {
  return invoke("openai_get_auth_url");
}

export async function openaiExchangeCode(
  code: string,
  receivedState: string
): Promise<void> {
  return invoke("openai_exchange_code", { code, receivedState });
}

export async function openaiLogout(): Promise<void> {
  return invoke("openai_logout");
}

// ============ Spotify Playback ============

export async function getCurrentTrack(): Promise<TrackInfo | null> {
  return invoke("get_current_track");
}

export async function getCurrentTrackWithAi(): Promise<TrackInfo | null> {
  return invoke("get_current_track_with_ai");
}

// ============ Settings ============

export async function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  return invoke("update_settings", { settings });
}

// ============ Auth Status ============

export async function getAuthStatus(): Promise<AuthStatus> {
  return invoke("get_auth_status");
}
