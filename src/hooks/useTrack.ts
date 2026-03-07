import { useState, useEffect, useCallback, useRef } from "react";
import type { TrackInfo } from "../types";
import { getCurrentTrack, getCurrentTrackWithAi, isSpotifyRunning } from "../lib/tauri";

const REGEN_COOLDOWN_MS = 5000;
type FetchAiSource = "auto" | "manual";

export interface FetchAiOptions {
  force?: boolean;
  source?: FetchAiSource;
}

export interface AiFetchEvent {
  nonce: number;
  source: FetchAiSource;
  trackId: string;
}

interface PendingFetch {
  force: boolean;
  source: FetchAiSource;
}

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
  const [aiError, setAiError] = useState<string | null>(null);
  const [lastAiFetch, setLastAiFetch] = useState<AiFetchEvent | null>(null);
  const [regenCooldown, setRegenCooldown] = useState(false);
  const lastTrackId = useRef<string | null>(null);
  const aiLoadingRef = useRef(false);
  const autoAiRef = useRef(autoAi);
  const regenTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingFetchRef = useRef<PendingFetch | null>(null);
  const fetchAiRef = useRef<(options?: FetchAiOptions) => Promise<void>>(async () => {});
  const fetchNonceRef = useRef(0);

  // Keep ref in sync with prop
  autoAiRef.current = autoAi;

  const fetchAi = useCallback(async (options: FetchAiOptions = {}) => {
    const { force = false, source = "manual" } = options;
    if (aiLoadingRef.current) {
      const pending = pendingFetchRef.current;
      pendingFetchRef.current = {
        force: (pending?.force ?? false) || force,
        source: pending?.source === "manual" || source === "manual" ? "manual" : "auto",
      };
      return;
    }
    aiLoadingRef.current = true;
    setAiLoading(true);
    const requestedTrackId = lastTrackId.current;
    if (force) {
      setRegenCooldown(true);
      if (regenTimerRef.current) clearTimeout(regenTimerRef.current);
      regenTimerRef.current = setTimeout(() => setRegenCooldown(false), REGEN_COOLDOWN_MS);
    }
    try {
      const aiTrack = await getCurrentTrackWithAi(force);
      setAiError(null);
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
        if (aiTrack.ai_description) {
          fetchNonceRef.current += 1;
          setLastAiFetch({
            nonce: fetchNonceRef.current,
            source,
            trackId: aiTrack.id,
          });
        }
      }
    } catch (err) {
      const errStr = err instanceof Error ? err.message : String(err);
      console.error("AI fetch failed:", errStr);
      setAiError(errStr);
    } finally {
      aiLoadingRef.current = false;
      setAiLoading(false);
      const pending = pendingFetchRef.current;
      pendingFetchRef.current = null;
      if (pending && lastTrackId.current && (pending.force || lastTrackId.current !== requestedTrackId)) {
        void fetchAiRef.current(pending);
      }
    }
  }, []);

  fetchAiRef.current = fetchAi;

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

          // Auto-fetch only for uncached tracks when enabled.
          if (autoAiRef.current && !localStorage.getItem(`ai_insight_${trackInfo.id}`)) {
            void fetchAi({ source: "auto" });
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

  // Clean up regen cooldown timer on unmount
  useEffect(() => {
    return () => {
      if (regenTimerRef.current) clearTimeout(regenTimerRef.current);
    };
  }, []);

  return {
    track,
    aiLoading,
    aiError,
    regenCooldown,
    lastAiFetch,
    error,
    spotifyRunning,
    fetchAi,
  };
}
