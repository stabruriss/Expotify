import { useState, useMemo, useEffect, useCallback, useRef, type PointerEvent, type MouseEvent, type WheelEvent, type KeyboardEvent } from "react";
import Markdown from "react-markdown";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize, LogicalPosition } from "@tauri-apps/api/dpi";
import { useTrack } from "../hooks/useTrack";
import { useLyrics } from "../hooks/useLyrics";
import { useReadAloud, type ReadAloudMode } from "../hooks/useReadAloud";
import { useSpeechService } from "../hooks/useSpeechService";
import { useLikeTrack } from "../hooks/useLikeTrack";
import { useAgentChat } from "../hooks/useAgentChat";
import { getUnreadChatReplies } from "../lib/chatReadAloud";
import { SpeechMusicPause } from "../lib/speechMusicPause";
import { getAuthStatus, getCurrentTrack, showMainWindow, saveOverlayGeometry, spotifyPlayPause, spotifyNextTrack, spotifyPreviousTrack, spotifyGetVolume, spotifySetVolume, spotifyShuffleLiked, spotifyPause, spotifyPlay, ttsSynthesize, getSettings, updateSettings } from "../lib/tauri";
import { useUpdateCheck } from "../hooks/useUpdateCheck";
import { DevicePicker } from "../components/DevicePicker";
import { useIMEComposition } from "../hooks/useIMEComposition";
import { AgentChat } from "../components/AgentChat";
import frameImg from "./assets/frame.png";
import "./overlay.css";

/* Try to import generated assets; fall back gracefully */
let aiStampImg: string | undefined;
try { aiStampImg = new URL("./assets/ai-stamp.png", import.meta.url).href; } catch {}
let aiChatBtnImg: string | undefined;
try { aiChatBtnImg = new URL("./assets/ai-chat-btn.png", import.meta.url).href; } catch {}

/** Strip markdown formatting for natural TTS reading. */
function stripMarkdown(md: string): string {
  return md
    .replace(/#{1,6}\s*/g, "")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/\*(.+?)\*/g, "$1")
    .replace(/__(.+?)__/g, "$1")
    .replace(/_(.+?)_/g, "$1")
    .replace(/~~(.+?)~~/g, "$1")
    .replace(/`(.+?)`/g, "$1")
    .replace(/\[(.+?)\]\(.+?\)/g, "$1")
    .replace(/!\[.*?\]\(.+?\)/g, "")
    .replace(/^[-*+]\s+/gm, "")
    .replace(/^\d+\.\s+/gm, "")
    .replace(/^>\s*/gm, "")
    .replace(/---+/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

const VISIBLE_LINES = 7;
const HALF = Math.floor(VISIBLE_LINES / 2);

function getLineClass(offset: number): string {
  const abs = Math.abs(offset);
  if (abs === 0) return "current";
  if (abs === 1) return "near";
  if (abs === 2) return "far";
  return "farthest";
}

function getLedClass(aiLoading: boolean, hasContent: boolean): string {
  if (aiLoading) return "led loading";
  if (hasContent) return "led active";
  return "led idle";
}

function isInteractiveTarget(target: HTMLElement): boolean {
  return !!target.closest(
    'button, a, input, textarea, select, [role="button"], [contenteditable="true"], [data-no-drag="true"]'
  );
}

export default function OverlayApp() {
  const { onCompositionEnd: imeCompositionEnd, isIMEEnter } = useIMEComposition();
  const { error: speechError, checking: speechChecking, available: speechAvailable, checkAvailability, reportError: reportSpeechError, dismissError: dismissSpeechError } = useSpeechService();
  const [speechMusic] = useState(() => new SpeechMusicPause({ getTrack: getCurrentTrack, pause: spotifyPause, play: spotifyPlay }));

  // Overlay-local auto-read mode. Migrate the legacy boolean to "all".
  const [readAloudMode, setReadAloudMode] = useState<ReadAloudMode>(() => {
    const stored = localStorage.getItem("expotify_insight_read_mode");
    if (stored === "off" || stored === "fetched_only" || stored === "all") {
      return stored;
    }
    return localStorage.getItem("expotify_insight_read_enabled") === "true" ? "all" : "off";
  });
  const [readModeMenuOpen, setReadModeMenuOpen] = useState(false);
  const insightReadAttemptRef = useRef(0);
  const lastEnabledReadModeRef = useRef<Exclude<ReadAloudMode, "off">>(
    readAloudMode !== "off" ? readAloudMode :
      localStorage.getItem("expotify_insight_read_last_mode") === "all" ? "all" : "fetched_only"
  );

  const commitInsightReadMode = useCallback((nextMode: ReadAloudMode) => {
    if (nextMode !== "off") {
      lastEnabledReadModeRef.current = nextMode;
      localStorage.setItem("expotify_insight_read_last_mode", nextMode);
    }
    setReadAloudMode(nextMode);
    setReadModeMenuOpen(false);
    localStorage.setItem("expotify_insight_read_mode", nextMode);
    localStorage.removeItem("expotify_insight_read_enabled");
  }, []);

  const setInsightReadMode = useCallback(async (nextMode: ReadAloudMode) => {
    const attempt = ++insightReadAttemptRef.current;
    setReadModeMenuOpen(false);
    if (nextMode !== "off" && !(await checkAvailability())) return;
    if (attempt === insightReadAttemptRef.current) commitInsightReadMode(nextMode);
  }, [checkAvailability, commitInsightReadMode]);

  const toggleInsightRead = useCallback(() => {
    void setInsightReadMode(readAloudMode === "off" ? lastEnabledReadModeRef.current : "off");
  }, [readAloudMode, setInsightReadMode]);

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === "expotify_insight_read_mode" && (e.newValue === "off" || e.newValue === "fetched_only" || e.newValue === "all")) {
        void setInsightReadMode(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [setInsightReadMode]);

  const toggleInsightReadMenu = useCallback(() => {
    setReadModeMenuOpen((open) => !open);
  }, []);

  const isReadAloudActive = readAloudMode !== "off";

  // Auto-fetch AI insight toggle (synced from main window settings)
  const [autoAiEnabled, setAutoAiEnabled] = useState(
    () => localStorage.getItem("expotify_settings_ai_auto") === "true"
  );

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === "expotify_settings_ai_auto") {
        setAutoAiEnabled(e.newValue === "true");
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const { track, aiLoading, aiError, regenCooldown, spotifyRunning, initialized: trackInitialized, fetchAi, lastAiFetch } = useTrack({
    pollInterval: isReadAloudActive ? 1 : 3,
    autoAi: autoAiEnabled,
  });
  const { lyrics, currentLineIndex, loading: lyricsLoading, refetchLyrics } = useLyrics({ track });

  const { updateAvailable, latestVersion, openRelease, dismiss } = useUpdateCheck();

  const [collapsed, setCollapsed] = useState(false);
  const [geometryReady, setGeometryReady] = useState(false);
  const collapsedRef = useRef(false);
  const expandedGeoRef = useRef({ width: 420, height: 268 });
  const expandingRef = useRef(false);
  const collapseTransitionRef = useRef(false);
  const wasEmptyRef = useRef(false);

  type PanelType = "ai" | "chat" | "device" | null;
  const [activePanel, setActivePanel] = useState<PanelType>(null);
  const [cachedAi, setCachedAi] = useState<{ trackId: string | null; text: string | null }>({
    trackId: null,
    text: null,
  });
  const [isAuthenticated, setIsAuthenticated] = useState(false);
  const [spotifyAuthed, setSpotifyAuthed] = useState(false);
  const [shuffleLoading, setShuffleLoading] = useState(false);
  const [lyricsScrollOffset, setLyricsScrollOffset] = useState(0);
  const [spotifyVolume, setSpotifyVolume] = useState<number | null>(null);
  const [ttsVolume, setTtsVolume] = useState(() => {
    const stored = localStorage.getItem("expotify_settings_tts_volume");
    return stored ? parseFloat(stored) : 0.8;
  });
  const volumeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scrollResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scrollAccumRef = useRef(0);

  // Save overlay geometry on move/resize (restore is handled by Rust before window shows)
  useEffect(() => {
    const win = getCurrentWindow();

    const geo = { x: 0, y: 0, width: 420, height: 268 };
    let geoReady = false;
    let active = true;

    const flushGeo = () => {
      saveOverlayGeometry(geo.x, geo.y, geo.width, geo.height).catch((e) =>
        console.error("save geometry failed:", e)
      );
    };

    const persistGeo = () => {
      if (!geoReady) return;
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      saveTimerRef.current = setTimeout(flushGeo, 500);
    };

    // Read initial geometry from actual window (already positioned by Rust)
    // If saved geometry was too small (e.g. from a collapsed session), restore defaults
    // Also clamp position to screen bounds as a safety net
    const initGeo = async () => {
      try {
        const [pos, size, sf] = await Promise.all([win.outerPosition(), win.outerSize(), win.scaleFactor()]);
        if (!active) return;
        let w = size.width / sf;
        let h = size.height / sf;
        let x = pos.x / sf;
        let y = pos.y / sf;

        if (w < 200 || h < 120) {
          // Saved geometry was from a collapsed state — restore defaults
          await win.setMinSize(new LogicalSize(300, 180));
          await win.setResizable(true);
          await win.setSize(new LogicalSize(420, 268));
          w = 420;
          h = 268;
        }

        // Clamp position to keep overlay on-screen
        const screenW = window.screen.availWidth;
        const screenH = window.screen.availHeight;
        const screenX = (window.screen as any).availLeft ?? 0;
        const screenY = (window.screen as any).availTop ?? 0;
        let clamped = false;
        if (x + w < screenX + 50 || x > screenX + screenW - 50) {
          x = screenX + screenW - w - 32;
          clamped = true;
        }
        if (y + h < screenY + 50 || y > screenY + screenH - 50) {
          y = screenY + screenH - h - 32;
          clamped = true;
        }
        if (clamped) {
          await win.setPosition(new LogicalPosition(x, y));
        }

        geo.x = x;
        geo.y = y;
        geo.width = w;
        geo.height = h;
      } catch {}
      if (!active) return;
      geoReady = true;
      setGeometryReady(true);
    };

    const unlistenMove = win.onMoved(async ({ payload }) => {
      try {
        const sf = await win.scaleFactor();
        geo.x = payload.x / sf;
        geo.y = payload.y / sf;
        persistGeo();
      } catch {}
    });

    const unlistenResize = win.onResized(async ({ payload }) => {
      if (collapsedRef.current) return; // Don't save compact geometry
      try {
        const sf = await win.scaleFactor();
        geo.width = payload.width / sf;
        geo.height = payload.height / sf;
        persistGeo();
      } catch {}
    });

    void initGeo();

    // Flush geometry on page unload (best effort)
    const onBeforeUnload = () => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      flushGeo();
    };
    window.addEventListener("beforeunload", onBeforeUnload);

    return () => {
      active = false;
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      if (scrollResetRef.current) clearTimeout(scrollResetRef.current);
      flushGeo(); // Flush pending save on cleanup
      window.removeEventListener("beforeunload", onBeforeUnload);
      unlistenMove.then((fn) => fn());
      unlistenResize.then((fn) => fn());
    };
  }, []);

  // Check auth status on mount and periodically
  useEffect(() => {
    const checkAuth = () => {
      getAuthStatus()
        .then((status) => {
          setIsAuthenticated(status.openai || status.anthropic);
          setSpotifyAuthed(status.spotify);
        })
        .catch(() => {});
    };
    checkAuth();
    const interval = setInterval(checkAuth, 10000);
    return () => clearInterval(interval);
  }, []);

  const volumeChangedRef = useRef(0);

  // Poll Spotify volume periodically (skip if user recently changed it)
  useEffect(() => {
    if (!spotifyRunning) return;
    const fetchVol = () => {
      if (Date.now() - volumeChangedRef.current < 2000) return;
      spotifyGetVolume()
        .then((vol) => setSpotifyVolume(vol))
        .catch(() => {});
    };
    fetchVol();
    const interval = setInterval(fetchVol, 5000);
    return () => clearInterval(interval);
  }, [spotifyRunning]);

  // Sync TTS volume from main window via localStorage
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === "expotify_settings_tts_volume" && e.newValue) {
        setTtsVolume(parseFloat(e.newValue));
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const handleSpotifyVolumeChange = useCallback((vol: number) => {
    setSpotifyVolume(vol);
    volumeChangedRef.current = Date.now();
    if (volumeTimerRef.current) clearTimeout(volumeTimerRef.current);
    volumeTimerRef.current = setTimeout(() => {
      spotifySetVolume(vol).catch(() => {});
    }, 100);
  }, []);

  const handleTtsVolumeChange = useCallback((vol: number) => {
    setTtsVolume(vol);
    localStorage.setItem("expotify_settings_tts_volume", String(vol));
    // Persist to settings file (debounced)
    if (volumeTimerRef.current) clearTimeout(volumeTimerRef.current);
    volumeTimerRef.current = setTimeout(() => {
      getSettings().then((s) => {
        updateSettings({ ...s, tts_volume: vol }).catch(() => {});
      }).catch(() => {});
    }, 500);
  }, []);

  // If AI error indicates auth issue, open main window for login
  useEffect(() => {
    if (
      aiError &&
      (aiError.toLowerCase().includes("not authenticated") ||
        aiError.includes("401") ||
        aiError.toLowerCase().includes("unauthorized"))
    ) {
      setIsAuthenticated(false);
      showMainWindow();
    }
  }, [aiError]);

  useEffect(() => {
    if (track?.id) {
      const stored = localStorage.getItem(`ai_insight_${track.id}`);
      setCachedAi({ trackId: track.id, text: stored });
    } else {
      setCachedAi({ trackId: null, text: null });
    }
  }, [track?.id]);

  useEffect(() => {
    if (track?.ai_description && track.id) {
      localStorage.setItem(`ai_insight_${track.id}`, track.ai_description);
      setCachedAi({ trackId: track.id, text: track.ai_description });
      setActivePanel("ai");
    }
  }, [track?.ai_description, track?.id]);

  // Sync AI insight from other window via storage event
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (track?.id && e.key === `ai_insight_${track.id}`) {
        setCachedAi({ trackId: track.id, text: e.newValue });
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [track?.id]);

  const displayedAi =
    track?.ai_description ??
    (track?.id && cachedAi.trackId === track.id ? cachedAi.text : null);
  const displayedAiTrackId =
    track?.ai_description
      ? (track?.id ?? null)
      : (track?.id && cachedAi.trackId === track.id ? track.id : null);

  // Collapsed mode chat input
  const [collapsedChatOpen, setCollapsedChatOpen] = useState(false);
  const [collapsedChatInput, setCollapsedChatInput] = useState("");
  const collapsedInputRef = useRef<HTMLInputElement>(null);
  const likeChangedRef = useRef<() => void>(() => {});
  const { entries: chatEntries, sendMessage: chatSendMessage, loading: chatLoading, reset: chatReset, cancel: chatCancel } = useAgentChat({
    onLikeChanged: () => likeChangedRef.current(),
  });

  // Resize window when collapsed chat opens/closes (keep cover in place)
  useEffect(() => {
    if (!collapsed || expandingRef.current) return;
    const win = getCurrentWindow();
    const CHAT_W = 180;
    const CHAT_H = 110;
    if (collapsedChatOpen) {
      const resize = async () => {
        await win.setMinSize(new LogicalSize(CHAT_W, CHAT_H));
        // Check screen bounds — keep cover at same position
        try {
          const sf = await win.scaleFactor();
          const pos = await win.outerPosition();
          const x = pos.x / sf;
          const y = pos.y / sf;
          const screenW = window.screen.availWidth;
          const screenH = window.screen.availHeight;
          let newX = x;
          let newY = y;
          if (x + CHAT_W > screenW) newX = screenW - CHAT_W;
          if (y + CHAT_H > screenH) newY = screenH - CHAT_H;
          if (newX < 0) newX = 0;
          if (newY < 0) newY = 0;
          if (newX !== x || newY !== y) {
            await win.setPosition(new LogicalPosition(newX, newY));
          }
        } catch {}
        await win.setSize(new LogicalSize(CHAT_W, CHAT_H));
        setTimeout(() => collapsedInputRef.current?.focus(), 150);
      };
      resize();
    } else {
      win.setMinSize(new LogicalSize(72, 72)).then(() =>
        win.setSize(new LogicalSize(72, 72))
      );
    }
  }, [collapsed, collapsedChatOpen]);

  // ===== Chat TTS (read-aloud for agent chat responses) =====
  const [chatReadEnabled, setChatReadEnabled] = useState(
    () => localStorage.getItem("expotify_chat_read_enabled") === "true"
  );
  const [chatTtsSpeaking, setChatTtsSpeaking] = useState(false);
  const [chatTtsPaused, setChatTtsPaused] = useState(false);
  const chatReadAttemptRef = useRef(0);
  const chatTtsAudioRef = useRef<HTMLAudioElement | null>(null);
  const chatMusicOwnerRef = useRef<symbol | null>(null);
  const chatTtsBlobUrlRef = useRef<string | null>(null);
  const lastChatObservedIdRef = useRef(chatEntries[chatEntries.length - 1]?.id ?? 0);
  const chatTtsSessionRef = useRef(0);
  const chatTtsQueueRef = useRef<string[]>([]);
  const chatTtsPlayingRef = useRef(false);
  const isReadingRef = useRef(false);
  const ttsVolumeRef = useRef(ttsVolume);

  const cleanupChatTts = useCallback(() => {
    if (chatTtsAudioRef.current) {
      chatTtsAudioRef.current.pause();
      chatTtsAudioRef.current.src = "";
      chatTtsAudioRef.current = null;
    }
    if (chatTtsBlobUrlRef.current) {
      URL.revokeObjectURL(chatTtsBlobUrlRef.current);
      chatTtsBlobUrlRef.current = null;
    }
  }, []);

  const releaseChatMusic = useCallback(() => {
    const owner = chatMusicOwnerRef.current;
    chatMusicOwnerRef.current = null;
    return owner ? speechMusic.release(owner) : Promise.resolve();
  }, [speechMusic]);

  const skipChatTts = useCallback(() => {
    chatTtsSessionRef.current++;
    chatTtsQueueRef.current = [];
    chatTtsPlayingRef.current = false;
    cleanupChatTts();
    setChatTtsSpeaking(false);
    setChatTtsPaused(false);
    void releaseChatMusic().catch((e) => console.error("Failed to restore Spotify:", e));
  }, [cleanupChatTts, releaseChatMusic]);

  const toggleChatTtsPause = useCallback(() => {
    const audio = chatTtsAudioRef.current;
    if (!audio) return;
    if (audio.paused) {
      audio.play().catch((e) => {
        reportSpeechError(e);
        skipChatTts();
      });
      setChatTtsPaused(false);
    } else {
      audio.pause();
      setChatTtsPaused(true);
    }
  }, [reportSpeechError, skipChatTts]);

  const toggleChatRead = useCallback(async () => {
    const next = !chatReadEnabled;
    const attempt = ++chatReadAttemptRef.current;
    if (next && !(await checkAvailability())) return;
    if (attempt !== chatReadAttemptRef.current) return;
    setChatReadEnabled(next);
    localStorage.setItem("expotify_chat_read_enabled", String(next));
    if (!next) skipChatTts();
  }, [chatReadEnabled, skipChatTts, checkAvailability]);

  // Restored ON preferences also need a check before any automatic speech.
  const restoredReadPreferences = useRef({ insight: readAloudMode, chat: chatReadEnabled });
  useEffect(() => {
    if (restoredReadPreferences.current.insight === "off" && !restoredReadPreferences.current.chat) return;
    let active = true;
    void checkAvailability().then((ok) => {
      if (ok || !active) return;
      commitInsightReadMode("off");
      setChatReadEnabled(false);
      localStorage.setItem("expotify_chat_read_enabled", "false");
    });
    return () => { active = false; };
  }, [checkAvailability, commitInsightReadMode]);

  // Apply volume changes to active chat TTS audio
  useEffect(() => {
    if (chatTtsAudioRef.current) {
      chatTtsAudioRef.current.volume = ttsVolume;
    }
  }, [ttsVolume]);

  // Cleanup chat TTS on unmount
  useEffect(() => {
    return () => {
      cleanupChatTts();
      void releaseChatMusic().catch((e) => console.error("Failed to restore Spotify:", e));
    };
  }, [cleanupChatTts, releaseChatMusic]);

  // Read-aloud orchestration
  const { phase: readAloudPhase, skipReadAloud, toggleSpeechPause, speechPaused, toggleManualRead, isAutoTriggered } = useReadAloud({
    music: speechMusic,
    mode: speechAvailable ? readAloudMode : "off",
    autoFetchEnabled: autoAiEnabled,
    track,
    displayedAi,
    displayedAiTrackId,
    aiLoading,
    lastAiFetch,
    ttsVolume,
    onError: reportSpeechError,
  });
  const isReading = readAloudPhase !== "idle";
  const isInsightPlaybackActive = isReading && readAloudPhase !== "fetching_ai";
  const isAnySpeaking = isReading || chatTtsSpeaking;
  const prevReadingRef = useRef(false);

  // Keep refs in sync for use inside async closures
  isReadingRef.current = isReading;
  ttsVolumeRef.current = ttsVolume;

  // Process chat TTS queue — speaks items one by one, resumes Spotify when done
  const processQueue = useCallback(async () => {
    if (chatTtsPlayingRef.current) return; // already processing
    if (chatTtsQueueRef.current.length === 0) return;
    if (isReadingRef.current) return; // wait for AI insight to finish

    chatTtsPlayingRef.current = true;
    const session = ++chatTtsSessionRef.current;
    setChatTtsSpeaking(true);
    setChatTtsPaused(false);

    const owner = Symbol("Chat speech");
    chatMusicOwnerRef.current = owner;
    // Share the pause with Insight and preserve already-paused music.
    try {
      await speechMusic.hold(owner);
    } catch (e) {
      if (chatMusicOwnerRef.current === owner) chatMusicOwnerRef.current = null;
      if (chatTtsSessionRef.current !== session) return;
      reportSpeechError(e);
      chatTtsQueueRef.current = [];
      chatTtsPlayingRef.current = false;
      setChatTtsSpeaking(false);
      return;
    }
    if (chatTtsSessionRef.current !== session) return;

    while (chatTtsQueueRef.current.length > 0) {
      if (chatTtsSessionRef.current !== session) return;
      // If AI insight started reading, pause queue processing
      if (isReadingRef.current) break;

      const rawText = chatTtsQueueRef.current.shift()!;
      const text = stripMarkdown(rawText);

      try {
        const base64Audio = await ttsSynthesize(text);
        if (chatTtsSessionRef.current !== session) return;

        const binaryStr = atob(base64Audio);
        const bytes = new Uint8Array(binaryStr.length);
        for (let i = 0; i < binaryStr.length; i++) {
          bytes[i] = binaryStr.charCodeAt(i);
        }
        const blob = new Blob([bytes], { type: "audio/mp3" });
        const url = URL.createObjectURL(blob);
        if (chatTtsBlobUrlRef.current) URL.revokeObjectURL(chatTtsBlobUrlRef.current);
        chatTtsBlobUrlRef.current = url;

        await new Promise<void>((resolve, reject) => {
          if (chatTtsSessionRef.current !== session) { resolve(); return; }
          const audio = new Audio(url);
          audio.volume = ttsVolumeRef.current;
          chatTtsAudioRef.current = audio;
          audio.onended = () => resolve();
          audio.onerror = () => reject(new Error(audio.error?.message || "Could not play speech audio"));
          audio.play().catch(reject);
        });
      } catch (e) {
        if (chatTtsSessionRef.current !== session) return;
        console.error("Chat TTS error:", e);
        reportSpeechError(e);
        chatTtsQueueRef.current = [];
        cleanupChatTts();
        break;
      }

      if (chatTtsSessionRef.current !== session) return;
      cleanupChatTts();
    }

    if (chatTtsSessionRef.current === session) {
      chatTtsPlayingRef.current = false;
      setChatTtsSpeaking(false);
      setChatTtsPaused(false);
      try { await releaseChatMusic(); } catch (e) { console.error("Failed to restore Spotify:", e); }
    }
  }, [cleanupChatTts, reportSpeechError, releaseChatMusic, speechMusic]);

  // Watch for new chat assistant messages → add to TTS queue
  useEffect(() => {
    if (chatEntries.length === 0) return;

    const replies = getUnreadChatReplies(chatEntries, lastChatObservedIdRef.current);
    // Advance even while disabled, so enabling Auto Read does not replay old replies.
    lastChatObservedIdRef.current = chatEntries[chatEntries.length - 1].id;
    if (!chatReadEnabled || !speechAvailable || replies.length === 0) return;

    // React can batch the reply and a trailing system status into one update.
    chatTtsQueueRef.current.push(...replies.map((entry) => entry.content));

    // Start processing if not already running and AI insight not reading
    if (!chatTtsPlayingRef.current && !isReadingRef.current) {
      void processQueue();
    }
  }, [chatEntries, chatReadEnabled, speechAvailable, processQueue]);

  // When AI insight finishes reading → process pending chat TTS queue
  useEffect(() => {
    if (!isReading && chatTtsQueueRef.current.length > 0 && !chatTtsPlayingRef.current) {
      processQueue();
    }
  }, [isReading, processQueue]);

  // Like track
  const { liked, loading: likeLoading, rateLimited, toggleLike, refreshLikeStatus } = useLikeTrack({
    trackId: track?.id ?? null,
    spotifyAuthenticated: spotifyAuthed,
  });

  likeChangedRef.current = refreshLikeStatus;

  // Shuffle liked songs
  const handleShuffleLiked = useCallback(async () => {
    if (shuffleLoading) return;
    setShuffleLoading(true);
    try {
      await spotifyShuffleLiked();
    } catch (e) {
      console.error("Shuffle liked failed:", e);
    } finally {
      setShuffleLoading(false);
    }
  }, [shuffleLoading]);

  // Close AI panel when auto-read finishes (was reading → now idle, auto-triggered)
  useEffect(() => {
    const wasReading = prevReadingRef.current;
    prevReadingRef.current = isReading;
    if (wasReading && !isReading && isAutoTriggered) {
      setActivePanel(null);
    }
  }, [isReading, isAutoTriggered]);

  useEffect(() => {
    if (activePanel !== "ai" && readModeMenuOpen) {
      setReadModeMenuOpen(false);
    }
  }, [activePanel, readModeMenuOpen]);

  const handleAiInsight = useCallback(async () => {
    if (!isAuthenticated) {
      showMainWindow();
      return;
    }
    if (activePanel === "ai") {
      setActivePanel(null);
    } else if (displayedAi || aiError || aiLoading) {
      setActivePanel("ai");
    } else {
      setActivePanel("ai");
      await fetchAi({ source: "manual" });
    }
  }, [activePanel, aiError, aiLoading, displayedAi, fetchAi, isAuthenticated]);

  const handleRegenerate = useCallback(async () => {
    await fetchAi({ force: true, source: "manual" });
  }, [fetchAi]);

  const autoReadSummary =
    readAloudMode === "fetched_only"
      ? "New fetched"
      : readAloudMode === "all"
        ? "Cached + new"
        : "Off";

  const handleMouseDown = useCallback(
    async (event: MouseEvent<HTMLElement>) => {
      if (event.button !== 0) return;
      const target = event.target as HTMLElement | null;
      if (!target || isInteractiveTarget(target)) return;
      try {
        await getCurrentWindow().startDragging();
      } catch (error) {
        console.error("startDragging failed", error);
      }
    },
    []
  );

  /* Native OS resize via Tauri startResizeDragging */
  const handleResizeSW = useCallback(
    async (event: PointerEvent<HTMLElement>) => {
      event.preventDefault();
      event.stopPropagation();
      try {
        await (getCurrentWindow() as any).startResizeDragging("SouthWest");
      } catch (error) {
        console.error("resize SW failed", error);
      }
    },
    []
  );

  const handleResizeSE = useCallback(
    async (event: PointerEvent<HTMLElement>) => {
      event.preventDefault();
      event.stopPropagation();
      try {
        await (getCurrentWindow() as any).startResizeDragging("SouthEast");
      } catch (error) {
        console.error("resize SE failed", error);
      }
    },
    []
  );

  const handleOpenMain = useCallback(() => {
    showMainWindow();
  }, []);

  const setOverlayCollapsed = useCallback(async (nextCollapsed: boolean) => {
    if (collapseTransitionRef.current || collapsedRef.current === nextCollapsed) return;
    collapseTransitionRef.current = true;
    const previousCollapsed = collapsedRef.current;
    try {
      const win = getCurrentWindow();
      const sf = await win.scaleFactor();

      if (nextCollapsed) {
        const size = await win.outerSize();
        expandedGeoRef.current = { width: size.width / sf, height: size.height / sf };
        // Close collapsed chat if open
        setCollapsedChatOpen(false);
        setCollapsedChatInput("");
        // Mark collapsed BEFORE resizing so the resize handler skips saving 72x72
        collapsedRef.current = true;
        await win.setMinSize(new LogicalSize(72, 72));
        await win.setResizable(false);
        await win.setSize(new LogicalSize(72, 72));
      } else {
        // Prevent the collapsed-chat resize effect from racing with expand
        expandingRef.current = true;
        setCollapsedChatOpen(false);
        setCollapsedChatInput("");
        const { width, height } = expandedGeoRef.current;
        await win.setSize(new LogicalSize(width, height));
        await win.setMinSize(new LogicalSize(300, 180));
        await win.setResizable(true);

        // Clamp position so the expanded window stays on screen
        try {
          const pos = await win.outerPosition();
          const wX = pos.x / sf;
          const wY = pos.y / sf;
          const screenW = window.screen.availWidth;
          const screenH = window.screen.availHeight;
          let newX = wX;
          let newY = wY;
          if (wX + width > screenW) newX = screenW - width;
          if (wY + height > screenH) newY = screenH - height;
          if (newX < 0) newX = 0;
          if (newY < 0) newY = 0;
          if (newX !== wX || newY !== wY) {
            await win.setPosition(new LogicalPosition(newX, newY));
          }
        } catch {}
        // Mark expanded AFTER resizing so subsequent events save the correct size
        collapsedRef.current = false;
        expandingRef.current = false;
      }
      setCollapsed(nextCollapsed);
    } catch (error) {
      collapsedRef.current = previousCollapsed;
      console.error("change overlay size failed:", error);
    } finally {
      expandingRef.current = false;
      collapseTransitionRef.current = false;
    }
  }, []);

  const handleToggleCollapse = useCallback(() => {
    void setOverlayCollapsed(!collapsedRef.current);
  }, [setOverlayCollapsed]);

  // Collapse once when entering an empty state, after the first successful poll.
  // Repeated empty polls must not undo a user's manual expansion.
  useEffect(() => {
    if (!trackInitialized || !geometryReady) return;
    const empty = !track;
    const enteredEmpty = empty && !wasEmptyRef.current;
    wasEmptyRef.current = empty;
    if (empty) {
      setActivePanel(null);
      setCollapsedChatOpen(false);
      setCollapsedChatInput("");
    }
    if (enteredEmpty && !speechError) void setOverlayCollapsed(true);
  }, [trackInitialized, geometryReady, track, speechError, setOverlayCollapsed]);

  // Errors need an expanded native window as well as an expanded layout.
  useEffect(() => {
    if (speechError && geometryReady) void setOverlayCollapsed(false);
  }, [speechError, geometryReady, collapsed, setOverlayCollapsed]);

  /* Scroll through lyrics with mouse wheel / trackpad */
  const handleLyricsWheel = useCallback((e: WheelEvent<HTMLElement>) => {
    if (!lyrics?.synced_lines?.length) return;
    const totalLines = lyrics.synced_lines.length;
    const actualCenter = currentLineIndex >= 0 ? currentLineIndex : 0;

    // Accumulate deltaY to tame trackpad's many tiny events
    scrollAccumRef.current += e.deltaY;
    const THRESHOLD = 50; // pixels of accumulated scroll per line
    const lines = Math.trunc(scrollAccumRef.current / THRESHOLD);
    if (lines === 0) {
      // Not enough accumulated yet — still schedule the snap-back timer
      if (scrollResetRef.current) clearTimeout(scrollResetRef.current);
      scrollResetRef.current = setTimeout(() => {
        setLyricsScrollOffset(0);
        scrollAccumRef.current = 0;
      }, 3000);
      return;
    }
    scrollAccumRef.current -= lines * THRESHOLD;

    setLyricsScrollOffset((prev) => {
      const next = prev + lines;
      const minOff = -actualCenter;
      const maxOff = totalLines - 1 - actualCenter;
      return Math.max(minOff, Math.min(maxOff, next));
    });

    // Auto-snap back to current line after 3s of no scrolling
    if (scrollResetRef.current) clearTimeout(scrollResetRef.current);
    scrollResetRef.current = setTimeout(() => {
      setLyricsScrollOffset(0);
      scrollAccumRef.current = 0;
    }, 3000);
  }, [lyrics, currentLineIndex]);

  const visibleLines = useMemo(() => {
    if (!lyrics?.synced_lines?.length) return [];
    const lines = lyrics.synced_lines;
    const translations = lyrics.translation_lines;
    const actualCenter = currentLineIndex >= 0 ? currentLineIndex : 0;
    // viewCenter shifts the visible window; style is still based on actualCenter
    const viewCenter = Math.max(0, Math.min(lines.length - 1, actualCenter + lyricsScrollOffset));
    const result: Array<{
      text: string;
      translation?: string;
      offset: number;
      viewOffset: number;
      index: number;
      spacer?: boolean;
    }> = [];
    // Always produce VISIBLE_LINES entries so the current line stays at slot HALF (center)
    for (let slot = 0; slot < VISIBLE_LINES; slot++) {
      const i = viewCenter - HALF + slot;
      const viewOffset = slot - HALF;
      if (i < 0 || i >= lines.length) {
        // Invisible spacer — preserves vertical position of center line
        result.push({ text: "", offset: Infinity, viewOffset, index: -(slot + 1), spacer: true });
        continue;
      }
      const translation = translations?.find(
        (t) => Math.abs(t.time_ms - lines[i].time_ms) < 500
      );
      result.push({
        text: lines[i].text,
        translation: translation?.text,
        offset: i - actualCenter, // distance from actual playing line (for is-playing)
        viewOffset,               // distance from viewport center (for size class)
        index: i,
      });
    }
    return result;
  }, [lyrics, currentLineIndex, lyricsScrollOffset]);

  /* Update notification bar */
  const speechErrorNotice = speechError ? (
    <div className="overlay-speech-error" role="alert" data-no-drag="true">
      <span className="overlay-speech-error-icon" aria-hidden="true">!</span>
      <span>{speechError}</span>
      <button onClick={dismissSpeechError} title="Dismiss read-aloud error" aria-label="Dismiss read-aloud error">×</button>
    </div>
  ) : null;

  const updateBar = updateAvailable ? (
    <div
      className="overlay-update-bar"
      data-no-drag="true"
      inert={activePanel !== null && !collapsed && spotifyRunning && !!track}
    >
      <button className="overlay-update-btn" onClick={openRelease}>
        v{latestVersion} available
      </button>
      <button className="overlay-update-dismiss" onClick={dismiss} title="Dismiss">
        &times;
      </button>
    </div>
  ) : null;

  /* Resize handles - watercolor dot indicators at both bottom corners */
  const resizeHandles = (
    <>
      <div
        className="overlay-resize-handle overlay-resize-sw"
        data-no-drag="true"
        onPointerDown={handleResizeSW}
        onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      >
        <div className="overlay-resize-dot" />
      </div>
      <div
        className="overlay-resize-handle overlay-resize-se"
        data-no-drag="true"
        onPointerDown={handleResizeSE}
        onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      >
        <div className="overlay-resize-dot" />
      </div>
    </>
  );

  /* Vinyl cover element (rotates) */
  const coverElement = (
    <div className={`overlay-cover-wrapper ${track?.is_playing ? "playing" : ""}`}>
      <div className="overlay-vinyl-disc" />
      {track?.album_art_url ? (
        <img className="overlay-cover" src={track.album_art_url} alt="" draggable={false} />
      ) : (
        <div className="overlay-cover-placeholder" />
      )}
    </div>
  );

  /* Cover area: non-rotating wrapper with control buttons overlay */
  const coverArea = (
    <div className={`overlay-cover-area${track ? "" : " overlay-cover-area-empty"}`}>
      {coverElement}
      {/* Quill writing animation during AI fetch */}
      {track && readAloudPhase === "fetching_ai" && (
        <div className="overlay-pen-writing">
          <img className="quill-img" src="/quill.png" alt="" />
        </div>
      )}
      {/* Top-left: collapse/expand */}
      <button
        className="overlay-cover-btn overlay-btn-tl"
        data-no-drag="true"
        onClick={handleToggleCollapse}
        title={collapsed ? "Expand" : "Collapse"}
        aria-label={collapsed ? "Expand" : "Collapse"}
        aria-expanded={!collapsed}
        disabled={!geometryReady}
      >
        <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="rgba(51,166,184,0.85)" strokeWidth="1.8" strokeLinecap="round">
          {collapsed ? <path d="M2 5L4 3L6 5" /> : <path d="M2 3L4 5L6 3" />}
        </svg>
      </button>
      {track && (
        <>
          {/* Top-right: play/pause (or TTS pause/resume during any read-aloud) */}
          <button
            className={`overlay-cover-btn overlay-btn-tr${isAnySpeaking ? " reading-active" : ""}`}
            data-no-drag="true"
            onClick={() => {
              if (chatTtsSpeaking) {
                toggleChatTtsPause();
              } else if (isReading) {
                toggleSpeechPause();
              } else {
                spotifyPlayPause().catch(() => {});
              }
            }}
            title={isAnySpeaking ? ((speechPaused || chatTtsPaused) ? "Resume reading" : "Pause reading") : (track.is_playing ? "Pause" : "Play")}
          >
            {isAnySpeaking ? (
              (speechPaused || chatTtsPaused) ? (
                <svg width="7" height="7" viewBox="0 0 8 8" fill="#FEDFE1">
                  <polygon points="2.5,1.5 6.5,4 2.5,6.5" />
                </svg>
              ) : (
                <svg width="7" height="7" viewBox="0 0 8 8" fill="#FEDFE1">
                  <rect x="2" y="1.5" width="1.5" height="5" rx="0.4" />
                  <rect x="4.5" y="1.5" width="1.5" height="5" rx="0.4" />
                </svg>
              )
            ) : track.is_playing ? (
              <svg width="7" height="7" viewBox="0 0 8 8" fill="rgba(51,166,184,0.85)">
                <rect x="2" y="1.5" width="1.5" height="5" rx="0.4" />
                <rect x="4.5" y="1.5" width="1.5" height="5" rx="0.4" />
              </svg>
            ) : (
              <svg width="7" height="7" viewBox="0 0 8 8" fill="rgba(51,166,184,0.85)">
                <polygon points="2.5,1.5 6.5,4 2.5,6.5" />
              </svg>
            )}
          </button>
          {/* Bottom-left: previous */}
          <button
            className="overlay-cover-btn overlay-btn-bl"
            data-no-drag="true"
            onClick={() => {
              if (chatTtsSpeaking) skipChatTts();
              else if (isReading) skipReadAloud();
              spotifyPreviousTrack().catch(() => {});
            }}
            title="Previous"
          >
            <svg width="7" height="7" viewBox="0 0 8 8" fill="rgba(51,166,184,0.85)">
              <polygon points="4.5,1.5 1.5,4 4.5,6.5" />
              <line x1="1.2" y1="1.5" x2="1.2" y2="6.5" stroke="rgba(51,166,184,0.85)" strokeWidth="1.2" />
            </svg>
          </button>
          {/* Bottom-right: next (or skip read-aloud during any TTS) */}
          <button
            className={`overlay-cover-btn overlay-btn-br${isAnySpeaking ? " reading-active" : ""}`}
            data-no-drag="true"
            onClick={() => {
              if (chatTtsSpeaking) {
                skipChatTts();
              } else if (isReading) {
                skipReadAloud();
              } else {
                spotifyNextTrack().catch(() => {});
              }
            }}
            title={isAnySpeaking ? "Skip read-aloud" : "Next"}
          >
            <svg width="7" height="7" viewBox="0 0 8 8" fill={isAnySpeaking ? "#FEDFE1" : "rgba(51,166,184,0.85)"}>
              <polygon points="3.5,1.5 6.5,4 3.5,6.5" />
              <line x1="6.8" y1="1.5" x2="6.8" y2="6.5" stroke={isAnySpeaking ? "#FEDFE1" : "rgba(51,166,184,0.85)"} strokeWidth="1.2" />
            </svg>
          </button>
        </>
      )}
    </div>
  );

  if (collapsed) {
    const handleCollapsedChatSend = () => {
      const text = collapsedChatInput.trim();
      if (!text || chatLoading) return;
      setCollapsedChatInput("");
      chatSendMessage(text);
      setCollapsedChatOpen(false);
    };
    const handleCollapsedKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter" && !e.shiftKey && !isIMEEnter()) {
        e.preventDefault();
        handleCollapsedChatSend();
      }
      if (e.key === "Escape") {
        setCollapsedChatOpen(false);
      }
    };

    return (
      <div className="overlay-frame overlay-collapsed" onMouseDown={handleMouseDown}>
        {speechErrorNotice}
        <div className="overlay-compact-content">
          {coverArea}
          {/* Sakura AI button at center of cover */}
          {track && isAuthenticated && spotifyAuthed && (
            <button
              className="overlay-collapsed-ai-btn"
              data-no-drag="true"
              onClick={() => setCollapsedChatOpen(!collapsedChatOpen)}
              title="Quick chat"
            >
              {aiChatBtnImg ? (
                <img src={aiChatBtnImg} alt="Chat" draggable={false} style={{ width: 15, height: 15 }} />
              ) : (
                <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2 3h12v8H6l-4 3V3z" />
                </svg>
              )}
            </button>
          )}
        </div>
        {/* Collapsed chat input */}
        {track && collapsedChatOpen && (
          <div className="overlay-collapsed-chat" data-no-drag="true">
            <input
              ref={collapsedInputRef}
              className="overlay-collapsed-chat-input"
              value={collapsedChatInput}
              onChange={(e) => setCollapsedChatInput(e.target.value)}
              onKeyDown={handleCollapsedKeyDown}
              onCompositionEnd={imeCompositionEnd}
              placeholder="Ask anything..."
              disabled={chatLoading}
            />
            <button
              className="overlay-collapsed-chat-send"
              onClick={handleCollapsedChatSend}
              disabled={!collapsedChatInput.trim() || chatLoading}
            >
              <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
                <path d="M2 14l12-6L2 2v5l8 1-8 1v5z" />
              </svg>
            </button>
          </div>
        )}
      </div>
    );
  }

  if (!track || !spotifyRunning) {
    return (
      <div className="overlay-frame" onMouseDown={handleMouseDown}>
        {speechErrorNotice}
        <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />
        <div className="overlay-content">
          {updateBar}
          <div className="overlay-header overlay-empty-header">{coverArea}</div>
          <div className="overlay-not-playing">{spotifyRunning ? "No track playing" : "Spotify is not running"}</div>
        </div>
        {resizeHandles}
      </div>
    );
  }

  return (
    <div className="overlay-frame" onMouseDown={handleMouseDown}>
      {speechErrorNotice}
      {/* SVG filter for AI panel organic edges */}
      <svg style={{ position: "absolute", width: 0, height: 0 }} aria-hidden="true">
        <defs>
          <filter id="rough-edge">
            <feTurbulence type="fractalNoise" baseFrequency="0.03" numOctaves="4" seed="3" result="noise" />
            <feDisplacementMap in="SourceGraphic" in2="noise" scale="20" xChannelSelector="R" yChannelSelector="G" />
          </filter>
        </defs>
      </svg>
      <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />

      <div className="overlay-content">
        {updateBar}
        {/* Header: vinyl cover + song info + open button + AI stamp */}
        <div className="overlay-header" inert={activePanel !== null}>
          {coverArea}
          <div className="overlay-meta">
            <div className="overlay-track-name">{track.name}</div>
            <div className="overlay-track-artist">{track.artist}</div>
          </div>
          {/* Open main window button - inline in header */}
          <button
            className="overlay-open-main"
            data-no-drag="true"
            onClick={handleOpenMain}
            title="Open Expotify"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M6 3H4a1.5 1.5 0 00-1.5 1.5v7A1.5 1.5 0 004 13h8a1.5 1.5 0 001.5-1.5V9.5" />
              <path d="M9.5 2.5h4v4" />
              <path d="M13.5 2.5L8 8" />
            </svg>
          </button>
          <div className="overlay-ai-toggle" data-no-drag="true">
            <button
              className={aiStampImg ? "overlay-ai-btn" : "overlay-ai-btn-text"}
              onClick={handleAiInsight}
              disabled={aiLoading && !displayedAi && !aiError}
            >
              {aiStampImg ? (
                <img className="overlay-ai-stamp" src={aiStampImg} alt="AI" draggable={false} />
              ) : (
                "AI"
              )}
            </button>
            <span className={getLedClass(aiLoading, !!displayedAi || !!aiError)} />
          </div>
        </div>

        {/* Player controls are covered while a panel is open. */}
        <div className="overlay-control-bar" data-no-drag="true" inert={activePanel !== null}>
          <div className="overlay-volume">
            <svg className="overlay-volume-icon" width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 2.5L4.5 5.5H2v5h2.5L8 13.5V2.5z" />
              {(spotifyVolume ?? 100) > 0 && <path d="M10.5 5.5a3.5 3.5 0 010 5" fill="none" stroke="currentColor" strokeWidth="1.2" />}
              {(spotifyVolume ?? 100) > 50 && <path d="M12 3.5a6 6 0 010 9" fill="none" stroke="currentColor" strokeWidth="1.2" />}
            </svg>
            <input
              type="range"
              className="overlay-volume-slider"
              min={0}
              max={100}
              value={spotifyVolume ?? 100}
              onChange={(e) => handleSpotifyVolumeChange(Number(e.target.value))}
            />
          </div>
          <div className="overlay-control-btns">
            {spotifyAuthed && (
              <>
                <span style={{ position: "relative" }}>
                  <button
                    className={`overlay-ctrl-btn${liked ? " liked" : ""}`}
                    onClick={toggleLike}
                    disabled={likeLoading}
                    title={liked ? "Unlike" : "Like"}
                  >
                    <svg width="17" height="17" viewBox="0 0 16 16" fill={liked ? "currentColor" : "none"} stroke="currentColor" strokeWidth="1.5">
                      <path d="M8 14s-5.5-3.5-5.5-7A3.5 3.5 0 018 4.5 3.5 3.5 0 0113.5 7C13.5 10.5 8 14 8 14z" />
                    </svg>
                  </button>
                  {rateLimited && (
                    <span style={{
                      position: "absolute",
                      top: "-18px",
                      left: "50%",
                      transform: "translateX(-50%)",
                      fontSize: "9px",
                      color: "rgba(254,223,225,0.60)",
                      background: "rgba(0,0,0,0.7)",
                      padding: "1px 5px",
                      borderRadius: "3px",
                      whiteSpace: "nowrap",
                      pointerEvents: "none",
                    }}>
                      Rate limited
                    </span>
                  )}
                </span>
                <button
                  className="overlay-ctrl-btn"
                  onClick={handleShuffleLiked}
                  disabled={shuffleLoading}
                  title="Shuffle liked songs"
                >
                  <svg width="17" height="17" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M2 4h3l3 8h4" />
                    <path d="M2 12h3l3-8h4" />
                    <path d="M13 3l2 1.5-2 1.5" />
                    <path d="M13 11l2 1.5-2 1.5" />
                  </svg>
                </button>
              </>
            )}
            {spotifyAuthed && (
              <button
                className="overlay-ctrl-btn"
                onClick={() => setActivePanel(activePanel === "device" ? null : "device")}
                title="Switch device"
              >
                <svg width="17" height="17" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="2" y="3" width="12" height="8" rx="1" />
                  <path d="M5 14h6" />
                  <path d="M8 11v3" />
                </svg>
              </button>
            )}
            {spotifyAuthed && isAuthenticated && (
              <button
                className="overlay-ctrl-btn"
                onClick={() => setActivePanel(activePanel === "chat" ? null : "chat")}
                title="AI Chat"
              >
                <svg width="17" height="17" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2 3h12v8H6l-4 3V3z" />
                  <path d="M5 6h6" />
                  <path d="M5 8.5h4" />
                </svg>
              </button>
            )}
          </div>
        </div>

        {aiError && (
          <div
            data-no-drag="true"
            style={{
              margin: "2px 0 8px",
              color: "rgba(255, 121, 121, 0.92)",
              fontSize: "10px",
              lineHeight: 1.35,
            }}
          >
            {aiError}
          </div>
        )}

        {/* Body: lyrics */}
        <div className="overlay-body" onWheel={handleLyricsWheel} inert={activePanel !== null}>
          {/* Refresh lyrics button */}
          <div className="overlay-btn-group" data-no-drag="true">
            <button
              className="overlay-action-btn"
              onClick={refetchLyrics}
              disabled={lyricsLoading}
              title="Refresh lyrics"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
                className={lyricsLoading ? "spinning" : ""}
              >
                <path d="M2 8a6 6 0 0110.47-4" />
                <path d="M14 8a6 6 0 01-10.47 4" />
                <path d="M12.47 1v3h-3" />
                <path d="M3.53 15v-3h3" />
              </svg>
            </button>
          </div>
          {/* Lyrics */}
          <div className="overlay-lyrics">
            {lyrics?.is_instrumental ? (
              <div className="overlay-no-lyrics">Instrumental</div>
            ) : visibleLines.length > 0 ? (
              visibleLines.map((line) =>
                line.spacer ? (
                  <div key={line.index} className="overlay-lyrics-line farthest" style={{ visibility: "hidden" }}>
                    &nbsp;
                  </div>
                ) : (
                  <div
                    key={line.index}
                    className={`overlay-lyrics-line ${getLineClass(line.viewOffset)}${line.offset === 0 ? " is-playing" : ""}`}
                  >
                    {line.text || "..."}
                    {line.translation && (
                      <div className="overlay-lyrics-translation">
                        {line.translation}
                      </div>
                    )}
                  </div>
                )
              )
            ) : lyrics?.plain_lyrics ? (
              <div className="overlay-no-lyrics">
                {lyrics.plain_lyrics
                  .split("\n")
                  .slice(0, 5)
                  .map((line, i) => (
                    <div key={i} className="overlay-lyrics-line near">
                      {line}
                    </div>
                  ))}
              </div>
            ) : (
              <div className="overlay-no-lyrics">No lyrics available</div>
            )}
          </div>
        </div>

        {/* Panels share the full inner frame, independently of the lyrics layout. */}
        <div className="overlay-panel-layer">
          {/* AI Insight panel */}
          {activePanel === "ai" && (
            <div className="overlay-panel" data-no-drag="true">
              <div className="overlay-panel-bg" />
              <div className="overlay-panel-content overlay-ai-panel-content" data-no-drag="true">
                <div className="overlay-ai-header">
                  <span className="overlay-ai-title">AI Insight</span>
                  <div className="overlay-tts-volume" data-no-drag="true">
                    <input
                      type="range"
                      className="overlay-tts-slider"
                      min={0}
                      max={100}
                      aria-label="Read-aloud volume"
                      title="Read-aloud volume"
                      value={Math.round(ttsVolume * 100)}
                      onChange={(e) => handleTtsVolumeChange(Number(e.target.value) / 100)}
                    />
                  </div>
                  <button className="overlay-ai-close" onClick={() => setActivePanel(null)} title="Close" aria-label="Close insight">
                    <svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
                      <path d="M2 2l8 8M10 2l-8 8" />
                    </svg>
                  </button>
                </div>
                <div className="overlay-ai-controls">
                  <button
                    className={`overlay-ai-read-btn${isInsightPlaybackActive ? " reading" : ""}`}
                    data-no-drag="true"
                    onClick={toggleManualRead}
                    disabled={!displayedAi && !isInsightPlaybackActive}
                    aria-pressed={isInsightPlaybackActive}
                    title={isInsightPlaybackActive ? "Stop this read-aloud" : "Read this insight once"}
                  >
                    {isInsightPlaybackActive ? "Stop" : "Read"}
                  </button>
                  <div className={`overlay-ai-read-mode overlay-auto-read-control${isReadAloudActive ? " active" : ""}`} data-no-drag="true">
                    <button
                      className="overlay-auto-read-switch"
                      onClick={toggleInsightRead}
                      disabled={speechChecking}
                      role="switch"
                      aria-checked={isReadAloudActive}
                      aria-label="Auto Read"
                      aria-busy={speechChecking}
                      title={speechChecking ? "Checking speech service" : `Auto read: ${autoReadSummary}`}
                    >
                      {speechChecking ? "Checking..." : <>Auto Read <span className="overlay-read-switch-state" aria-hidden="true">{isReadAloudActive ? "ON" : "OFF"}</span></>}
                    </button>
                    <button
                      className="overlay-ai-read-menu-trigger"
                      onClick={toggleInsightReadMenu}
                      disabled={speechChecking}
                      aria-expanded={readModeMenuOpen}
                      aria-haspopup="true"
                      title="Auto read options"
                    >
                      {isReadAloudActive && <span>{readAloudMode === "all" ? "All" : "New"}</span>}
                      <span aria-hidden="true">▾</span>
                    </button>
                    {readModeMenuOpen && (
                      <div className="overlay-ai-read-menu" data-no-drag="true">
                        <button
                          className={`overlay-ai-read-menu-item${readAloudMode === "off" ? " active" : ""}`}
                          onClick={() => setInsightReadMode("off")}
                          aria-label="Off"
                          aria-pressed={readAloudMode === "off"}
                        >
                          <span>Off</span><small>Manual reading only</small>
                        </button>
                        <button
                          className={`overlay-ai-read-menu-item${readAloudMode === "fetched_only" ? " active" : ""}`}
                          onClick={() => setInsightReadMode("fetched_only")}
                          aria-label="New fetched"
                          aria-pressed={readAloudMode === "fetched_only"}
                        >
                          <span>New fetched</span><small>Read newly generated insights</small>
                        </button>
                        <button
                          className={`overlay-ai-read-menu-item${readAloudMode === "all" ? " active" : ""}`}
                          onClick={() => setInsightReadMode("all")}
                          aria-label="Cached + new"
                          aria-pressed={readAloudMode === "all"}
                        >
                          <span>Cached + new</span><small>Include cached insights</small>
                        </button>
                      </div>
                    )}
                  </div>
                </div>
                <div className="overlay-ai-text">
                  {displayedAi ? (
                    <>
                      <Markdown>{displayedAi}</Markdown>
                      {track?.ai_used_web_search && (
                        <span className="overlay-ai-web-badge">web</span>
                      )}
                    </>
                  ) : aiLoading ? (
                    <p className="overlay-ai-placeholder">Generating AI insight...</p>
                  ) : aiError ? (
                    <p className="overlay-ai-error">{aiError}</p>
                  ) : (
                    <p className="overlay-ai-placeholder">No AI insight yet for this track.</p>
                  )}
                </div>
                <div className="overlay-ai-footer">
                  <button
                    className="overlay-ai-regen"
                    onClick={handleRegenerate}
                    disabled={aiLoading || regenCooldown}
                  >
                    {aiLoading
                      ? "Generating..."
                      : regenCooldown
                        ? "Cooldown..."
                        : displayedAi
                          ? "Regenerate"
                          : aiError
                            ? "Retry"
                            : "Generate"}
                  </button>
                </div>
              </div>
            </div>
          )}

          {/* Chat panel */}
          {activePanel === "chat" && (
            <div className="overlay-panel" data-no-drag="true">
              <div className="overlay-panel-bg" />
              <button className="overlay-panel-close" onClick={() => setActivePanel(null)} title="Close">
                <svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
                  <path d="M2 2l8 8M10 2l-8 8" />
                </svg>
              </button>
              <div className="overlay-panel-content overlay-panel-content-fixed" data-no-drag="true">
                <AgentChat
                  onClose={() => setActivePanel(null)}
                  entries={chatEntries}
                  loading={chatLoading}
                  sendMessage={chatSendMessage}
                  reset={chatReset}
                  cancel={chatCancel}
                  chatReadEnabled={chatReadEnabled}
                  chatReadChecking={speechChecking}
                  onToggleChatRead={toggleChatRead}
                  ttsVolume={ttsVolume}
                  onTtsVolumeChange={handleTtsVolumeChange}
                />
              </div>
            </div>
          )}

          {/* Device picker panel */}
          {activePanel === "device" && (
            <div className="overlay-panel" data-no-drag="true">
              <div className="overlay-panel-bg" />
              <button className="overlay-panel-close" onClick={() => setActivePanel(null)} title="Close">
                <svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
                  <path d="M2 2l8 8M10 2l-8 8" />
                </svg>
              </button>
              <div className="overlay-panel-content overlay-panel-content-fixed" data-no-drag="true">
                <DevicePicker onClose={() => setActivePanel(null)} />
              </div>
            </div>
          )}
        </div>
      </div>

      {resizeHandles}
    </div>
  );
}
