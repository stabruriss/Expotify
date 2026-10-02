import { useState, useEffect, useRef, useCallback } from "react";
import type { TrackInfo } from "../types";
import { getCurrentTrack, ttsSynthesize } from "../lib/tauri";
import type { SpeechMusicPause } from "../lib/speechMusicPause";
import type { AiFetchEvent } from "./useTrack";

export type ReadAloudPhase =
  | "idle"
  | "pausing"
  | "fetching_ai"
  | "speaking"
  | "resuming";

export type ReadAloudMode = "off" | "fetched_only" | "all";

interface UseReadAloudOptions {
  music: SpeechMusicPause;
  mode: ReadAloudMode;
  autoFetchEnabled: boolean;
  track: TrackInfo | null;
  displayedAi: string | null;
  displayedAiTrackId: string | null;
  aiLoading: boolean;
  lastAiFetch: AiFetchEvent | null;
  ttsVolume?: number;
  onError?: (error: unknown) => void;
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
  const { music, mode, autoFetchEnabled, track, displayedAi, displayedAiTrackId, aiLoading, lastAiFetch, ttsVolume = 0.8 } = options;
  const ttsVolumeRef = useRef(ttsVolume);
  const onErrorRef = useRef(options.onError);
  onErrorRef.current = options.onError;

  const [phase, setPhase] = useState<ReadAloudPhase>("idle");
  const [speechPaused, setSpeechPaused] = useState(false);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const musicOwnerRef = useRef<symbol | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const lastObservedTrackIdRef = useRef<string | null>(null);
  const phaseRef = useRef<ReadAloudPhase>("idle");
  const activeTrackIdRef = useRef<string | null>(null);
  const sessionRef = useRef(0);
  const isAutoTriggeredRef = useRef(false);
  const pendingFetchTrackIdRef = useRef<string | null>(null);
  const initialTrackIdRef = useRef<string | null>(null);
  const lastHandledFetchNonceRef = useRef(0);

  phaseRef.current = phase;
  ttsVolumeRef.current = ttsVolume;

  useEffect(() => {
    if (audioRef.current) {
      audioRef.current.volume = ttsVolume;
    }
  }, [ttsVolume]);

  const updatePhase = useCallback((nextPhase: ReadAloudPhase) => {
    phaseRef.current = nextPhase;
    setPhase(nextPhase);
  }, []);

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

  const releaseMusic = useCallback(() => {
    const owner = musicOwnerRef.current;
    musicOwnerRef.current = null;
    return owner ? music.release(owner) : Promise.resolve();
  }, [music]);

  const stopAudio = useCallback(() => {
    sessionRef.current++;
    cleanupAudio();
    pendingFetchTrackIdRef.current = null;
    updatePhase("idle");
    setSpeechPaused(false);
    void releaseMusic().catch((e) => console.error("Failed to restore Spotify:", e));
  }, [cleanupAudio, updatePhase, releaseMusic]);

  const resumeAndReset = useCallback(async (session: number) => {
    if (sessionRef.current !== session) return;
    updatePhase("resuming");
    cleanupAudio();
    try {
      await releaseMusic();
    } catch (e) {
      console.error("Failed to resume Spotify:", e);
    }
    if (sessionRef.current !== session) return;
    updatePhase("idle");
    setSpeechPaused(false);
  }, [updatePhase, cleanupAudio, releaseMusic]);

  const verifyCurrentTrack = useCallback(async (expectedTrackId: string, session: number) => {
    if (sessionRef.current !== session) return false;
    try {
      const currentTrack = await getCurrentTrack();
      if (sessionRef.current !== session) return false;
      return currentTrack?.id === expectedTrackId;
    } catch (e) {
      console.error("Failed to verify current track before read-aloud:", e);
      return sessionRef.current === session && activeTrackIdRef.current === expectedTrackId;
    }
  }, []);

  const speakText = useCallback(
    async (markdown: string, session: number) => {
      if (sessionRef.current !== session) return;
      updatePhase("speaking");
      const plainText = stripMarkdown(markdown);

      try {
        const base64Audio = await ttsSynthesize(plainText);
        if (sessionRef.current !== session) return;

        const binaryStr = atob(base64Audio);
        const bytes = new Uint8Array(binaryStr.length);
        for (let i = 0; i < binaryStr.length; i++) {
          bytes[i] = binaryStr.charCodeAt(i);
        }
        const blob = new Blob([bytes], { type: "audio/mp3" });
        const url = URL.createObjectURL(blob);
        blobUrlRef.current = url;

        await new Promise<void>((resolve, reject) => {
          if (sessionRef.current !== session) {
            resolve();
            return;
          }
          const audio = new Audio(url);
          audio.volume = ttsVolumeRef.current;
          audioRef.current = audio;
          audio.onended = () => resolve();
          audio.onerror = () => reject(new Error(audio.error?.message || "Could not play speech audio"));
          audio.play().then(() => undefined).catch(reject);
        });
      } catch (e) {
        if (sessionRef.current !== session) return;
        console.error("[ReadAloud] TTS error:", e);
        onErrorRef.current?.(e);
      }

      if (sessionRef.current !== session) return;
      cleanupAudio();
      await resumeAndReset(session);
    },
    [updatePhase, cleanupAudio, resumeAndReset]
  );

  const beginRead = useCallback(async (markdown: string, trackId: string, autoTriggered: boolean, session: number) => {
    if (!(await verifyCurrentTrack(trackId, session))) {
      if (sessionRef.current === session) updatePhase("idle");
      return;
    }
    isAutoTriggeredRef.current = autoTriggered;
    const owner = Symbol("Insight speech");
    musicOwnerRef.current = owner;
    updatePhase("pausing");
    try {
      await music.hold(owner);
    } catch (e) {
      console.error("Failed to pause Spotify:", e);
      if (musicOwnerRef.current === owner) musicOwnerRef.current = null;
      if (sessionRef.current === session) onErrorRef.current?.(e);
      if (sessionRef.current === session) updatePhase("idle");
      return;
    }
    if (sessionRef.current !== session) return;
    await speakText(markdown, session);
  }, [music, speakText, updatePhase, verifyCurrentTrack]);

  // Track changed while reading: stop immediately.
  useEffect(() => {
    if (!track?.id) return;
    const prevTrack = activeTrackIdRef.current;
    activeTrackIdRef.current = track.id;

    if (phaseRef.current !== "idle" && prevTrack && prevTrack !== track.id) {
      stopAudio();
    }
  }, [track?.id, stopAudio]);

  // Track change: in "all" mode, auto-read cached insights for future tracks only.
  useEffect(() => {
    if (!track?.id) return;

    if (lastObservedTrackIdRef.current === null) {
      lastObservedTrackIdRef.current = track.id;
      initialTrackIdRef.current = track.id;
      return;
    }

    if (track.id === lastObservedTrackIdRef.current) return;
    lastObservedTrackIdRef.current = track.id;

    if (mode === "all") {
      const cached = localStorage.getItem(`ai_insight_${track.id}`);
      if (cached) {
        stopAudio();
        const session = ++sessionRef.current;
        void beginRead(cached, track.id, true, session);
        return;
      }
    }

    if (mode !== "off" && autoFetchEnabled && !localStorage.getItem(`ai_insight_${track.id}`)) {
      pendingFetchTrackIdRef.current = track.id;
      updatePhase("fetching_ai");
    }
  }, [track?.id, mode, autoFetchEnabled, stopAudio, beginRead, updatePhase]);

  // Fresh fetch completed: auto-read both manual and auto fetches when mode is on.
  useEffect(() => {
    if (!lastAiFetch) return;
    if (lastAiFetch.nonce === lastHandledFetchNonceRef.current) return;

    if (mode === "off") {
      lastHandledFetchNonceRef.current = lastAiFetch.nonce;
      return;
    }

    if (!displayedAi || !displayedAiTrackId || !track?.id) return;
    if (track.id !== lastAiFetch.trackId || displayedAiTrackId !== lastAiFetch.trackId) return;

    lastHandledFetchNonceRef.current = lastAiFetch.nonce;
    pendingFetchTrackIdRef.current = null;

    if (lastAiFetch.source === "auto" && initialTrackIdRef.current === lastAiFetch.trackId) {
      initialTrackIdRef.current = null;
      if (phaseRef.current === "fetching_ai") updatePhase("idle");
      return;
    }

    stopAudio();
    const session = ++sessionRef.current;
    void beginRead(displayedAi, lastAiFetch.trackId, lastAiFetch.source === "auto", session);
  }, [mode, displayedAi, displayedAiTrackId, track?.id, lastAiFetch, stopAudio, beginRead, updatePhase]);

  // Clear the fetching phase if the pending auto-fetch did not produce a result.
  useEffect(() => {
    if (phaseRef.current !== "fetching_ai") return;
    if (mode === "off") {
      pendingFetchTrackIdRef.current = null;
      updatePhase("idle");
      return;
    }
    if (aiLoading) return;
    const pendingTrackId = pendingFetchTrackIdRef.current;
    if (!pendingTrackId) {
      updatePhase("idle");
      return;
    }
    if (lastAiFetch?.trackId === pendingTrackId) return;
    if (displayedAiTrackId === pendingTrackId && displayedAi) return;
    pendingFetchTrackIdRef.current = null;
    updatePhase("idle");
  }, [mode, aiLoading, displayedAi, displayedAiTrackId, lastAiFetch, updatePhase]);

  const skipReadAloud = useCallback(() => {
    stopAudio();
  }, [stopAudio]);

  const toggleSpeechPause = useCallback(() => {
    if (phaseRef.current !== "speaking") return;
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) {
      audio.play().catch((e) => {
        onErrorRef.current?.(e);
        skipReadAloud();
      });
      setSpeechPaused(false);
    } else {
      audio.pause();
      setSpeechPaused(true);
    }
  }, [skipReadAloud]);

  const toggleManualRead = useCallback(async () => {
    if (phaseRef.current !== "idle" && phaseRef.current !== "fetching_ai") {
      skipReadAloud();
      return;
    }
    if (!displayedAi || !track?.id) return;

    isAutoTriggeredRef.current = false;
    stopAudio();
    const session = ++sessionRef.current;
    await beginRead(displayedAi, track.id, false, session);
  }, [displayedAi, track?.id, skipReadAloud, stopAudio, beginRead]);

  useEffect(() => {
    return () => {
      cleanupAudio();
      void releaseMusic().catch((e) => console.error("Failed to restore Spotify:", e));
    };
  }, [cleanupAudio, releaseMusic]);

  return {
    phase,
    skipReadAloud,
    toggleSpeechPause,
    speechPaused,
    toggleManualRead,
    isAutoTriggered: isAutoTriggeredRef.current,
  };
}
