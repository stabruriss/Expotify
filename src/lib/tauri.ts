import { invoke } from "@tauri-apps/api/core";
import type { TrackInfo, Settings, AuthStatus, LyricsInfo, SearchResult, SpotifyDevice, ChatMessage, AgentChatResult } from "../types";

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

export async function spotifyPause(): Promise<void> {
  return invoke("spotify_pause");
}

export async function spotifyPlay(): Promise<void> {
  return invoke("spotify_play");
}

// ============ TTS ============

export async function ttsSynthesize(text: string): Promise<string> {
  return invoke("tts_synthesize", { text });
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

// ============ Anthropic ============

export async function anthropicStartOAuth(): Promise<void> {
  return invoke("anthropic_start_oauth");
}

export async function anthropicCompleteOAuth(code: string): Promise<void> {
  return invoke("anthropic_complete_oauth", { code });
}

export async function anthropicCancelOAuth(): Promise<void> {
  return invoke("anthropic_cancel_oauth");
}

export async function anthropicLogout(): Promise<void> {
  return invoke("anthropic_logout");
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

// ============ Spotify Web API ============

export async function spotifyIsAuthenticated(): Promise<boolean> {
  return invoke("spotify_is_authenticated");
}

export async function spotifyConnect(spDc: string): Promise<void> {
  return invoke("spotify_connect", { spDc });
}

export async function spotifyLogin(): Promise<void> {
  return invoke("spotify_login");
}

export async function spotifyDisconnect(): Promise<void> {
  return invoke("spotify_disconnect");
}

export async function spotifySearch(query: string, limit?: number): Promise<SearchResult[]> {
  return invoke("spotify_search", { query, limit });
}

export async function spotifyIsTrackLiked(trackId: string): Promise<boolean> {
  return invoke("spotify_is_track_liked", { trackId });
}

export async function spotifyLikeTrack(trackId: string): Promise<void> {
  return invoke("spotify_like_track", { trackId });
}

export async function spotifyUnlikeTrack(trackId: string): Promise<void> {
  return invoke("spotify_unlike_track", { trackId });
}

export async function spotifyShuffleLiked(): Promise<void> {
  return invoke("spotify_shuffle_liked");
}

export async function spotifyGetDevices(): Promise<SpotifyDevice[]> {
  return invoke("spotify_get_devices");
}

export async function spotifyTransferPlayback(deviceId: string): Promise<void> {
  return invoke("spotify_transfer_playback", { deviceId });
}

export async function spotifyGetVolume(): Promise<number> {
  return invoke("spotify_get_volume");
}

export async function spotifySetVolume(volume: number): Promise<void> {
  return invoke("spotify_set_volume", { volume });
}

export async function spotifyPlayTrack(uri: string): Promise<void> {
  return invoke("spotify_play_track", { uri });
}

// ============ Models ============

export interface ModelInfo {
  id: string;
  name: string;
  provider: string;
  created_at: string;
}

export async function listModels(): Promise<ModelInfo[]> {
  return invoke("list_models");
}

// ============ Agent Chat ============

export async function agentChat(messages: ChatMessage[]): Promise<AgentChatResult> {
  return invoke("agent_chat", { messages });
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
