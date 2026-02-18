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
  ai_used_web_search: boolean;
}

export interface Settings {
  poll_interval_secs: number;
  show_ai_description: boolean;
  ai_model: string;
  ai_prompt: string;
  ai_web_search: boolean;
  ai_auto: boolean;
  window_position: [number, number] | null;
  window_opacity: number;
}

export interface AuthStatus {
  openai: boolean;
}

export const AVAILABLE_MODELS = [
  { id: "gpt-5.2", name: "GPT-5.2", desc: "Latest" },
  { id: "gpt-5.1", name: "GPT-5.1", desc: "" },
  { id: "gpt-5", name: "GPT-5", desc: "" },
] as const;

export const DEFAULT_AI_PROMPT = `Briefly introduce this song (under 500 words):

Song: {name}
Artist: {artist}
Album: {album}

Include the song's style/genre and creative background. Do not repeat the song title or artist name. Give the introduction directly without preamble. No citation links in the output.

Search online for interesting stories about the track, the creator, and details about this specific version and performer, and weave them into the introduction.`;

// Lyrics
export interface LyricsLine {
  time_ms: number;
  text: string;
}

export type LyricsSource = "NetEase" | "QQMusic" | "Kugou" | "Lrclib" | "PetitLyrics" | "None";

export interface LyricsInfo {
  track_id: string;
  is_instrumental: boolean;
  synced_lines: LyricsLine[];
  plain_lyrics: string | null;
  translation_lines: LyricsLine[];
  source: LyricsSource;
  fetch_log: string[];
}
