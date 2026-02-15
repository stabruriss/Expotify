// Track information from Spotify
export interface TrackInfo {
  id: string;
  name: string;
  artist: string;
  album: string;
  album_art_url: string | null;
  duration_ms: number;
  progress_ms: number;
  is_playing: boolean;
  spotify_url: string | null;
  ai_description: string | null;
}

// Settings
export interface Settings {
  poll_interval_secs: number;
  show_ai_description: boolean;
  ai_model: string;
  window_position: [number, number] | null;
  window_opacity: number;
}

// Auth status
export interface AuthStatus {
  spotify: boolean;
  openai: boolean;
}

// Lyrics
export interface LyricsLine {
  time_ms: number;
  text: string;
}

export type LyricsSource = "NetEase" | "Lrclib" | "PetitLyrics" | "None";

export interface LyricsInfo {
  track_id: string;
  is_instrumental: boolean;
  synced_lines: LyricsLine[];
  plain_lyrics: string | null;
  translation_lines: LyricsLine[];
  source: LyricsSource;
}
