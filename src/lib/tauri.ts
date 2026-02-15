import { invoke } from "@tauri-apps/api/core";
import type { TrackInfo, Settings, AuthStatus, LyricsInfo } from "../types";

// ============ Spotify Status ============

export async function isSpotifyRunning(): Promise<boolean> {
  return invoke("is_spotify_running");
}

// ============ OpenAI Auth ============

export async function openaiIsAuthenticated(): Promise<boolean> {
  return invoke("openai_is_authenticated");
}

export async function openaiLogin(): Promise<void> {
  return invoke("openai_login");
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

// ============ Lyrics ============

export async function getLyrics(
  trackId: string,
  trackName: string,
  artist: string,
  album: string,
  durationMs: number
): Promise<LyricsInfo> {
  return invoke("get_lyrics", {
    trackId,
    trackName,
    artist,
    album,
    durationMs,
  });
}
