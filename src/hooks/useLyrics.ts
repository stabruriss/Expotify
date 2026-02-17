import { useState, useEffect, useRef } from "react";
import type { TrackInfo, LyricsInfo } from "../types";
import { getLyrics } from "../lib/tauri";

interface UseLyricsOptions {
  track: TrackInfo | null;
}

export function useLyrics({ track }: UseLyricsOptions) {
  const [lyrics, setLyrics] = useState<LyricsInfo | null>(null);
  const [currentLineIndex, setCurrentLineIndex] = useState(-1);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastFetchedTrackId = useRef<string | null>(null);

  // Client-side progress interpolation
  const progressRef = useRef({ base: 0, timestamp: 0, isPlaying: false });

  // Fetch lyrics when track changes
  useEffect(() => {
    if (!track || track.id === lastFetchedTrackId.current) {
      return;
    }

    lastFetchedTrackId.current = track.id;
    setLoading(true);
    setError(null);
    setCurrentLineIndex(-1);
    setLyrics(null);

    getLyrics(track.id, track.name, track.artist, track.album, track.duration_ms)
      .then(setLyrics)
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setLyrics(null);
      })
      .finally(() => setLoading(false));
  }, [track?.id]);

  // Update progress reference when track polling updates (every 3s)
  useEffect(() => {
    if (track) {
      progressRef.current = {
        base: track.progress_ms,
        timestamp: Date.now(),
        isPlaying: track.is_playing,
      };
    }
  }, [track?.progress_ms, track?.is_playing]);

  // Client-side interpolation for synced lyrics
  useEffect(() => {
    if (!lyrics || !lyrics.synced_lines.length || !track) {
      return;
    }

    const updateCurrentLine = () => {
      const { base, timestamp, isPlaying } = progressRef.current;
      const elapsed = isPlaying ? Date.now() - timestamp : 0;
      const estimatedProgress = base + elapsed;

      // Find current line (last line whose time_ms <= estimatedProgress)
      const lines = lyrics.synced_lines;
      let idx = -1;
      for (let i = lines.length - 1; i >= 0; i--) {
        if (lines[i].time_ms <= estimatedProgress) {
          idx = i;
          break;
        }
      }

      setCurrentLineIndex(idx);
    };

    updateCurrentLine();
    const interval = setInterval(updateCurrentLine, 100);
    return () => clearInterval(interval);
  }, [lyrics, track?.id]);

  return { lyrics, currentLineIndex, loading, error };
}
