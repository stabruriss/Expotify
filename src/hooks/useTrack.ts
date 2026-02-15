import { useState, useEffect, useCallback, useRef } from "react";
import type { TrackInfo } from "../types";
import { getCurrentTrackWithAi, isSpotifyRunning } from "../lib/tauri";

interface UseTrackOptions {
  pollInterval?: number;
}

export function useTrack(options: UseTrackOptions = {}) {
  const { pollInterval = 3 } = options;

  const [track, setTrack] = useState<TrackInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [spotifyRunning, setSpotifyRunning] = useState(true);
  const lastTrackId = useRef<string | null>(null);

  const fetchTrack = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      const running = await isSpotifyRunning();
      setSpotifyRunning(running);

      if (!running) {
        setTrack(null);
        lastTrackId.current = null;
        return;
      }

      const trackInfo = await getCurrentTrackWithAi();

      if (trackInfo) {
        if (trackInfo.id !== lastTrackId.current) {
          lastTrackId.current = trackInfo.id;
          setTrack(trackInfo);
        } else {
          setTrack((prev) =>
            prev
              ? {
                  ...prev,
                  progress_ms: trackInfo.progress_ms,
                  is_playing: trackInfo.is_playing,
                  ai_description:
                    trackInfo.ai_description ?? prev.ai_description,
                  ai_used_web_search:
                    trackInfo.ai_description
                      ? trackInfo.ai_used_web_search
                      : prev.ai_used_web_search,
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
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchTrack();
    const interval = setInterval(fetchTrack, pollInterval * 1000);
    return () => clearInterval(interval);
  }, [fetchTrack, pollInterval]);

  return {
    track,
    loading,
    error,
    spotifyRunning,
    refetch: fetchTrack,
  };
}
