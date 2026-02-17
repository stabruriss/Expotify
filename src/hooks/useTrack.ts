import { useState, useEffect, useCallback, useRef } from "react";
import type { TrackInfo } from "../types";
import { getCurrentTrack, getCurrentTrackWithAi, isSpotifyRunning } from "../lib/tauri";

const AI_COOLDOWN_MS = 3000;

interface UseTrackOptions {
  pollInterval?: number;
  autoAi?: boolean;
}

export function useTrack(options: UseTrackOptions = {}) {
  const { pollInterval = 3, autoAi = false } = options;

  const [track, setTrack] = useState<TrackInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [spotifyRunning, setSpotifyRunning] = useState(true);
  const [aiLoading, setAiLoading] = useState(false);
  const lastTrackId = useRef<string | null>(null);
  const aiLoadingRef = useRef(false);
  const lastAiFetchTime = useRef(0);
  const autoAiRef = useRef(autoAi);

  // Keep ref in sync with prop
  autoAiRef.current = autoAi;

  const fetchAi = useCallback(async () => {
    const now = Date.now();
    if (aiLoadingRef.current || now - lastAiFetchTime.current < AI_COOLDOWN_MS) return;
    aiLoadingRef.current = true;
    lastAiFetchTime.current = now;
    setAiLoading(true);
    try {
      const aiTrack = await getCurrentTrackWithAi();
      if (aiTrack && aiTrack.id === lastTrackId.current) {
        setTrack((prev) =>
          prev && prev.id === aiTrack.id
            ? {
                ...prev,
                ai_description: aiTrack.ai_description,
                ai_used_web_search: aiTrack.ai_used_web_search,
              }
            : prev
        );
      }
    } catch (err) {
      console.error("AI fetch failed:", err);
    } finally {
      aiLoadingRef.current = false;
      setAiLoading(false);
    }
  }, []);

  const fetchTrack = useCallback(async () => {
    try {
      setError(null);

      const running = await isSpotifyRunning();
      setSpotifyRunning(running);

      if (!running) {
        setTrack(null);
        lastTrackId.current = null;
        return;
      }

      const trackInfo = await getCurrentTrack();

      if (trackInfo) {
        if (trackInfo.id !== lastTrackId.current) {
          lastTrackId.current = trackInfo.id;
          setTrack(trackInfo);

          // Auto-fetch AI if enabled
          if (autoAiRef.current) {
            fetchAi();
          }
        } else {
          setTrack((prev) =>
            prev
              ? {
                  ...prev,
                  progress_ms: trackInfo.progress_ms,
                  is_playing: trackInfo.is_playing,
                }
              : trackInfo
          );
        }
      } else {
        setTrack(null);
        lastTrackId.current = null;
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [fetchAi]);

  useEffect(() => {
    fetchTrack();
    const interval = setInterval(fetchTrack, pollInterval * 1000);
    return () => clearInterval(interval);
  }, [fetchTrack, pollInterval]);

  return {
    track,
    aiLoading,
    error,
    spotifyRunning,
    fetchAi,
  };
}
