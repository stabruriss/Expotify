import { useState, useEffect, useRef, useCallback } from "react";
import type { TrackInfo } from "../types";
import { spotifyPause, spotifyPlay, ttsSynthesize } from "../lib/tauri";

export type ReadAloudPhase =
  | "idle"
  | "pausing"
  | "fetching_ai"
  | "speaking"
  | "resuming";

interface UseReadAloudOptions {
  enabled: boolean;
  track: TrackInfo | null;
  displayedAi: string | null;
  aiLoading: boolean;
  fetchAi: (force?: boolean) => Promise<void>;
  ttsVolume?: number;
}

interface UseReadAloudReturn {
  phase: ReadAloudPhase;
  skipReadAloud: () => void;
  toggleSpeechPause: () => void;
  speechPaused: boolean;
  toggleManualRead: () => void;
  isAutoTriggered: boolean;
}

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

export function useReadAloud(options: UseReadAloudOptions): UseReadAloudReturn {
  const { enabled, track, displayedAi, aiLoading, fetchAi, ttsVolume = 0.8 } = options;
  const ttsVolumeRef = useRef(ttsVolume);

  const [phase, setPhase] = useState<ReadAloudPhase>("idle");
  const [speechPaused, setSpeechPaused] = useState(false);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const lastSpokenTrackId = useRef<string | null>(null);
  const phaseRef = useRef<ReadAloudPhase>("idle");
  const activeTrackIdRef = useRef<string | null>(null);
  const sessionRef = useRef(0);
  const isAutoTriggeredRef = useRef(false);
  const sawLoadingRef = useRef(false); // tracks if aiLoading was true since entering fetching_ai

  // Keep refs in sync
  phaseRef.current = phase;
  ttsVolumeRef.current = ttsVolume;

  // Apply volume changes to active audio
  useEffect(() => {
    if (audioRef.current) {
      audioRef.current.volume = ttsVolume;
    }
  }, [ttsVolume]);

  const updatePhase = useCallback((p: ReadAloudPhase) => {
    phaseRef.current = p;
    setPhase(p);
  }, []);

  /** Stop audio playback and revoke blob URL. Does NOT change phase or session. */
  const cleanupAudio = useCallback(() => {
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.src = "";
      audioRef.current = null;
    }
    if (blobUrlRef.current) {
      URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = null;
    }
  }, []);

  /** Invalidate all in-flight async operations, stop audio, reset state. Does NOT resume Spotify. */
  const stopAudio = useCallback(() => {
    sessionRef.current++;
    cleanupAudio();
    updatePhase("idle");
    setSpeechPaused(false);
  }, [cleanupAudio, updatePhase]);

  /** Resume Spotify and reset to idle. Only called when a flow finishes naturally. */
  const resumeAndReset = useCallback(async (session: number) => {
    if (sessionRef.current !== session) return;
    updatePhase("resuming");
    cleanupAudio();
    try {
      await spotifyPlay();
    } catch (e) {
      console.error("Failed to resume Spotify:", e);
    }
    if (sessionRef.current !== session) return;
    updatePhase("idle");
    setSpeechPaused(false);
  }, [updatePhase, cleanupAudio]);

  /** Synthesize and play TTS. Checks session at every async boundary. */
  const speakText = useCallback(
    async (markdown: string, session: number) => {
      console.log("[ReadAloud] speakText called, session:", session, "current:", sessionRef.current, "textLen:", markdown.length);
      if (sessionRef.current !== session) { console.log("[ReadAloud] speakText: session mismatch at start"); return; }
      updatePhase("speaking");
      const plainText = stripMarkdown(markdown);
      console.log("[ReadAloud] Stripped text length:", plainText.length);

      try {
        console.log("[ReadAloud] Calling ttsSynthesize...");
        const base64Audio = await ttsSynthesize(plainText);
        console.log("[ReadAloud] ttsSynthesize returned, audioLen:", base64Audio.length);
        if (sessionRef.current !== session) { console.log("[ReadAloud] speakText: session mismatch after TTS"); return; }

        // Decode base64 to blob and play via Audio element
        const binaryStr = atob(base64Audio);
        const bytes = new Uint8Array(binaryStr.length);
        for (let i = 0; i < binaryStr.length; i++) {
          bytes[i] = binaryStr.charCodeAt(i);
        }
        const blob = new Blob([bytes], { type: "audio/mp3" });
        const url = URL.createObjectURL(blob);
        blobUrlRef.current = url;

        console.log("[ReadAloud] Playing audio, volume:", ttsVolumeRef.current);
        await new Promise<void>((resolve, reject) => {
          if (sessionRef.current !== session) { resolve(); return; }
          const audio = new Audio(url);
          audio.volume = ttsVolumeRef.current;
          audioRef.current = audio;
          audio.onended = () => { console.log("[ReadAloud] Audio ended naturally"); resolve(); };
          audio.onerror = (e) => { console.error("[ReadAloud] Audio error:", e); reject(e); };
          audio.play().then(() => console.log("[ReadAloud] Audio play started")).catch(reject);
        });
      } catch (e) {
        if (sessionRef.current !== session) return;
        console.error("[ReadAloud] TTS error:", e);
      }

      if (sessionRef.current !== session) { console.log("[ReadAloud] speakText: session mismatch after play"); return; }
      console.log("[ReadAloud] Cleaning up and resuming");
      cleanupAudio();
      await resumeAndReset(session);
    },
    [updatePhase, cleanupAudio, resumeAndReset]
  );

  // ===== Track change: always stop current readAloud if track changed =====
  useEffect(() => {
    if (!track?.id) return;
    const prevTrack = activeTrackIdRef.current;
    activeTrackIdRef.current = track.id;

    if (phaseRef.current !== "idle" && prevTrack && prevTrack !== track.id) {
      // Track changed during active reading — stop immediately
      const wasAutoTriggered = isAutoTriggeredRef.current;
      stopAudio();
      // Resume Spotify only if auto-read won't immediately start a new flow
      if (!enabled || !wasAutoTriggered) {
        spotifyPlay().catch(() => {});
      }
    }
  }, [track?.id, stopAudio, enabled]);

  // ===== Auto trigger: track ID changes while enabled =====
  useEffect(() => {
    if (!enabled || !track?.id) return;

    // Skip the very first track detected (app just opened)
    if (lastSpokenTrackId.current === null) {
      lastSpokenTrackId.current = track.id;
      return;
    }

    if (track.id === lastSpokenTrackId.current) return;
    lastSpokenTrackId.current = track.id;

    // Start new auto-read flow
    isAutoTriggeredRef.current = true;
    stopAudio(); // invalidate any prior flow (no resume — we'll pause again)
    const session = ++sessionRef.current;

    const beginReadAloud = async (trackId: string) => {
      // Check localStorage cache first
      const cached = localStorage.getItem(`ai_insight_${trackId}`);
      if (cached) {
        // Cache hit — pause Spotify and speak immediately
        updatePhase("pausing");
        try {
          await spotifyPause();
        } catch (e) {
          console.error("Failed to pause Spotify:", e);
          if (sessionRef.current === session) updatePhase("idle");
          return;
        }
        if (sessionRef.current !== session) return;
        await speakText(cached, session);
        return;
      }

      // Cache miss — fetch AI without pausing (keep music playing)
      if (sessionRef.current !== session) return;
      sawLoadingRef.current = false;
      updatePhase("fetching_ai");
      fetchAi();
      // displayedAi effect below picks up the result and pauses before speaking
    };

    beginReadAloud(track.id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [track?.id, enabled]);

  // ===== Handle AI text arrival during fetching_ai phase =====
  useEffect(() => {
    if (phaseRef.current !== "fetching_ai") return;
    if (!displayedAi) return;
    const session = sessionRef.current;

    // Pause Spotify first, then speak
    const pauseAndSpeak = async () => {
      updatePhase("pausing");
      try {
        await spotifyPause();
      } catch (e) {
        console.error("Failed to pause Spotify:", e);
        if (sessionRef.current === session) updatePhase("idle");
        return;
      }
      if (sessionRef.current !== session) return;
      await speakText(displayedAi, session);
    };
    pauseAndSpeak();
  }, [displayedAi, speakText, updatePhase]);

  // ===== Handle AI fetch failure =====
  useEffect(() => {
    if (phaseRef.current !== "fetching_ai") return;
    if (aiLoading) {
      // Mark that we've seen loading start — only then can we detect failure
      sawLoadingRef.current = true;
      return;
    }
    // aiLoading is false: only treat as failure if loading was seen (started then stopped)
    if (!sawLoadingRef.current) return;
    if (!displayedAi) {
      updatePhase("idle");
    }
  }, [aiLoading, displayedAi, updatePhase]);

  // ===== User actions =====

  /** Stop reading entirely and resume Spotify. */
  const skipReadAloud = useCallback(() => {
    stopAudio();
    spotifyPlay().catch(() => {});
  }, [stopAudio]);

  /** Pause or resume TTS audio playback. */
  const toggleSpeechPause = useCallback(() => {
    if (phaseRef.current !== "speaking") return;
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) {
      audio.play();
      setSpeechPaused(false);
    } else {
      audio.pause();
      setSpeechPaused(true);
    }
  }, []);

  /** Manual toggle: idle → start reading displayedAi; non-idle → stop reading. */
  const toggleManualRead = useCallback(async () => {
    console.log("[ReadAloud] toggleManualRead called, phase:", phaseRef.current, "displayedAi:", !!displayedAi, "len:", displayedAi?.length);
    if (phaseRef.current !== "idle") {
      // Currently reading — stop
      console.log("[ReadAloud] Not idle, skipping read-aloud");
      skipReadAloud();
      return;
    }
    if (!displayedAi) {
      console.log("[ReadAloud] No displayedAi, returning");
      return;
    }

    // Start manual read
    isAutoTriggeredRef.current = false;
    stopAudio(); // ensure clean state
    const session = ++sessionRef.current;
    console.log("[ReadAloud] Starting manual read, session:", session);

    updatePhase("pausing");
    try {
      await spotifyPause();
    } catch (e) {
      console.error("[ReadAloud] Failed to pause Spotify:", e);
      if (sessionRef.current === session) updatePhase("idle");
      return;
    }
    if (sessionRef.current !== session) {
      console.log("[ReadAloud] Session mismatch after pause, aborting");
      return;
    }

    console.log("[ReadAloud] Calling speakText, session:", session);
    await speakText(displayedAi, session);
  }, [displayedAi, skipReadAloud, stopAudio, updatePhase, speakText]);

  // ===== Cleanup on unmount =====
  useEffect(() => {
    return () => {
      cleanupAudio();
    };
  }, [cleanupAudio]);

  return { phase, skipReadAloud, toggleSpeechPause, speechPaused, toggleManualRead, isAutoTriggered: isAutoTriggeredRef.current };
}
