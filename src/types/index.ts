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
  ai_read_aloud: boolean;
  window_position: [number, number] | null;
  window_opacity: number;
  tts_volume: number;
  chat_model: string;
  chat_prompt: string;
  anthropic_enabled: boolean;
  memories: string[];
}

export interface AuthStatus {
  openai: boolean;
  anthropic: boolean;
  anthropic_available: boolean;
  spotify: boolean;
}

export interface SearchResult {
  id: string;
  name: string;
  artist: string;
  album: string;
  album_art_url: string | null;
  duration_ms: number;
  uri: string;
}

export interface SpotifyDevice {
  id: string;
  name: string;
  device_type: string;
  is_active: boolean;
  volume_percent: number | null;
}

export const AVAILABLE_MODELS = [
  { id: "gpt-5.2", name: "GPT-5.2", desc: "Latest", provider: "openai" },
  { id: "gpt-5.1", name: "GPT-5.1", desc: "", provider: "openai" },
  { id: "gpt-5", name: "GPT-5", desc: "", provider: "openai" },
  { id: "claude-sonnet-4-5-20250514", name: "Claude Sonnet 4.5", desc: "", provider: "anthropic" },
  { id: "claude-opus-4-6", name: "Claude Opus 4.6", desc: "Latest", provider: "anthropic" },
] as const;

export const DEFAULT_AI_PROMPT = `Briefly introduce this song (under 500 words):

Song: {name}
Artist: {artist}
Album: {album}

Include the song's style/genre and creative background. Do not repeat the song title or artist name. Give the introduction directly without preamble. No citation links in the output.

Search online for interesting stories about the track, the creator, and details about this specific version and performer, and weave them into the introduction.

{memories}
Consult the user's memories above (if any) for personalized insights. Always reply in the user's language.`;

export const DEFAULT_CHAT_PROMPT = `You are the Expotify music assistant and the user's chat companion.

Current playback: {name} - {artist} ({album})
Current volume: {volume}%

{memories}

Available tools (reply with a single JSON object when using a tool):
- search_and_play(query): Search for a song and play the best match.
- like_current: Add current song to Liked Songs.
- unlike_current: Remove current song from Liked Songs.
- shuffle_liked: Randomly play a song from Liked Songs.
- set_volume(level): Set volume (0-100).
- save_memory(content): Save something about the user's preferences or interests.
- update_prompt(type, content): Update the AI Insight ("insight") or Chat ("chat") prompt.

Tool response format (JSON only, no markdown):
{"action": "<tool>", "args": {"<param>": <value>}, "message": "brief explanation"}

For normal conversation, just reply with plain text — no JSON needed.

IMPORTANT — Music playback intent:
When the user's intent is clearly to play music (they mention a song, artist, album, genre, mood, era, or any music-related request), DO NOT ask follow-up questions. Immediately use search_and_play with the best query you can construct from the information given. Only ask for clarification if the request is genuinely too ambiguous to form any search query (e.g. "play something" with zero context).

You can chat about any topic. Use web search when helpful for factual questions.
Use save_memory when you learn something about the user's preferences.
Consult the memories above for user preferences when relevant.
Always reply in the user's language.`;

// Agent Chat
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}

export interface AgentResponse {
  action: string;
  message: string;
  args?: Record<string, unknown>;
}

export interface AgentChatResult {
  response: AgentResponse;
  executed: boolean;
  track_name: string | null;
  error?: string;
}

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
