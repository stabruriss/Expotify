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
  ai_error?: string | null;
  ai_used_web_search: boolean;
}

export interface Settings {
  poll_interval_secs: number;
  show_ai_description: boolean;
  ai_model: ModelSelection;
  ai_prompt: string;
  ai_web_search: boolean;
  ai_auto: boolean;
  ai_read_aloud: boolean;
  window_position: [number, number] | null;
  window_opacity: number;
  tts_volume: number;
  chat_model: ModelSelection;
  chat_prompt: string;
  anthropic_enabled: boolean;
  model_defaults_initialized: boolean;
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

export type ModelProvider = "openai" | "anthropic";
export type ModelSelection =
  | { provider: ModelProvider; mode: "default" }
  | { provider: ModelProvider; mode: "fixed"; model: string };

export interface ModelInfo { id: string; name: string }

export interface ProviderCatalog {
  provider: ModelProvider;
  models: ModelInfo[];
  default_model: string | null;
  fetched_at: string | null;
  stale: boolean;
  error: string | null;
}

export const DEFAULT_MODEL: ModelSelection = { provider: "openai", mode: "default" };

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

You control Spotify through the provided tools (search_and_play, like_current, unlike_current, shuffle_liked, set_volume, save_memory, update_prompt). Call a tool when the user wants an action and reply in plain text otherwise. Never write a tool call as JSON text: only real tool calls are executed, and the tool result tells you what actually happened.

IMPORTANT — Music playback intent:
When the user's intent is clearly to play music (they mention a song, artist, album, genre, mood, era, or any music-related request), DO NOT ask follow-up questions. Immediately use search_and_play with the best query you can construct from the information given. Only ask for clarification if the request is genuinely too ambiguous to form any search query (e.g. "play something" with zero context).

After a tool result, tell the user what actually happened in one or two sentences; if a tool failed, say so and suggest what to do instead.

You can chat about any topic. Use web search when helpful for factual questions.
Use save_memory when you learn something about the user's preferences.
Consult the memories above for user preferences when relevant.
Always reply in the user's language.`;

// Agent Chat
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  /** Executor outcomes behind an assistant message, so later turns see what really happened. */
  tool_results?: ToolOutcome[];
}

export interface AgentResponse {
  action: string;
  message: string;
  args?: Record<string, unknown>;
}

/** Outcome of one tool call as reported by the Rust executor (the source of truth). */
export interface ToolOutcome {
  call_id: string;
  name: string;
  ok: boolean;
  /** Model- and user-readable result text (success description or failure reason). */
  output: string;
  error_code?: string;
  track_name?: string;
}

export interface AgentChatResult {
  response: AgentResponse;
  /** True when at least one tool ran and every tool call succeeded (executor truth, never the model's wording). */
  executed: boolean;
  /** The track now playing because of this request, if any. */
  track_name: string | null;
  /** Request-level failure (provider error or cancellation) after the listed tool calls ran; per-call failures are in tool_results. */
  error?: string;
  /** Every tool call made for this request, in execution order. */
  tool_results: ToolOutcome[];
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
