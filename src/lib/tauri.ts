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

export async function getCurrentTrackWithAi(force = false): Promise<TrackInfo | null> {
  return invoke("get_current_track_with_ai", { force });
}

// ============ Spotify Playback Control ============

export async function spotifyPlayPause(): Promise<void> {
  return invoke("spotify_play_pause");
}

export async function spotifyNextTrack(): Promise<void> {
  return invoke("spotify_next_track");
}

export async function spotifyPreviousTrack(): Promise<void> {
  return invoke("spotify_previous_track");
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

// ============ Window Commands ============

export async function showMainWindow(): Promise<void> {
  return invoke("show_main_window");
}

// ============ Overlay Geometry ============

export interface OverlayGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export async function saveOverlayGeometry(x: number, y: number, width: number, height: number): Promise<void> {
  return invoke("save_overlay_geometry", { x, y, width, height });
}

export async function loadOverlayGeometry(): Promise<OverlayGeometry | null> {
  return invoke("load_overlay_geometry");
}

// ============ Update Check ============

export interface UpdateInfo {
  has_update: boolean;
  latest_version: string;
  download_url: string;
  release_url: string;
}

export async function checkForUpdate(): Promise<UpdateInfo> {
  return invoke("check_for_update");
}

export async function openUrl(url: string): Promise<void> {
  return invoke("open_url", { url });
}

// ============ Lyrics ============

export async function getLyrics(
  trackId: string,
  trackName: string,
  artist: string,
  album: string,
  durationMs: number,
  force = false
): Promise<LyricsInfo> {
  return invoke("get_lyrics", {
    trackId,
    trackName,
    artist,
    album,
    durationMs,
    force,
  });
}
