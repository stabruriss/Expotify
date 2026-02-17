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

export const DEFAULT_AI_PROMPT = `请用中文简洁地介绍这首歌曲（100字以内）：

歌曲: {name}
艺术家: {artist}
专辑: {album}

介绍应包含：歌曲的风格/流派、创作背景或有趣的故事（如果知道的话）。不要重复歌曲名和艺术家名。直接给出介绍，不需要开头语。`;

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
