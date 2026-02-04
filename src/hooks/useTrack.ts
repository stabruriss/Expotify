import { useState, useEffect, useCallback, useRef } from "react";
import type { TrackInfo } from "../types";
import { getCurrentTrackWithAi } from "../lib/tauri";

interface UseTrackOptions {
  pollInterval?: number; // in seconds
  enabled?: boolean;
}

export function useTrack(options: UseTrackOptions = {}) {
  const { pollInterval = 3, enabled = true } = options;

  const [track, setTrack] = useState<TrackInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastTrackId = useRef<string | null>(null);

  const fetchTrack = useCallback(async () => {
    if (!enabled) return;

    try {
      setLoading(true);
      setError(null);

      const trackInfo = await getCurrentTrackWithAi();

      // Only update if track changed or it's the first fetch
      if (trackInfo) {
        if (trackInfo.id !== lastTrackId.current) {
          lastTrackId.current = trackInfo.id;
          setTrack(trackInfo);
        } else {
          // Update progress and playing state without triggering full re-render
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
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;

    // Initial fetch
    fetchTrack();

    // Set up polling
    const interval = setInterval(fetchTrack, pollInterval * 1000);

    return () => clearInterval(interval);
  }, [fetchTrack, pollInterval, enabled]);

  return {
    track,
    loading,
    error,
    refetch: fetchTrack,
  };
}
