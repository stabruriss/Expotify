import { useState, useEffect, useRef, useCallback } from "react";
import type { TrackInfo, LyricsInfo } from "../types";
import { getLyrics } from "../lib/tauri";

interface UseLyricsOptions {
  track: TrackInfo | null;
}

function loadCachedLyrics(trackId: string): LyricsInfo | null {
  try {
    const stored = localStorage.getItem(`lyrics_${trackId}`);
    if (stored) return JSON.parse(stored) as LyricsInfo;
  } catch {}
  return null;
}

function saveLyricsToCache(lyrics: LyricsInfo) {
  // Don't cache "not found" results (empty, non-instrumental)
  if (!lyrics.is_instrumental && !lyrics.synced_lines.length && !lyrics.plain_lyrics) return;
  try {
    localStorage.setItem(`lyrics_${lyrics.track_id}`, JSON.stringify(lyrics));
  } catch {}
}

export function useLyrics({ track }: UseLyricsOptions) {
  const [lyrics, setLyrics] = useState<LyricsInfo | null>(null);
  const [currentLineIndex, setCurrentLineIndex] = useState(-1);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastFetchedTrackId = useRef<string | null>(null);

  // Client-side progress interpolation
  const progressRef = useRef({ base: 0, timestamp: 0, isPlaying: false });

  // Core fetch function
  const doFetch = useCallback(async (t: TrackInfo, force: boolean) => {
    setLoading(true);
    setError(null);
    if (force) {
      setLyrics(null);
      setCurrentLineIndex(-1);
    }
    try {
      const result = await getLyrics(t.id, t.name, t.artist, t.album, t.duration_ms, force);
      setLyrics(result);
      saveLyricsToCache(result);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      // On fetch error, keep existing lyrics (from cache) if available
      if (force) setLyrics(null);
    } finally {
      setLoading(false);
    }
  }, []);

  // Fetch lyrics when track changes
  useEffect(() => {
    if (!track || track.id === lastFetchedTrackId.current) {
      return;
    }

    lastFetchedTrackId.current = track.id;
    setCurrentLineIndex(-1);

    // Try localStorage cache first
    const cached = loadCachedLyrics(track.id);
    if (cached) {
      setLyrics(cached);
      setLoading(false);
      setError(null);
      return;
    }

    // No cache - fetch from backend
    setLyrics(null);
    doFetch(track, false);
  }, [track?.id, doFetch]);

  // Manual refetch (force re-fetch from network, bypassing all caches)
  const refetchLyrics = useCallback(async () => {
    if (!track) return;
    lastFetchedTrackId.current = track.id;
    localStorage.removeItem(`lyrics_${track.id}`);
    await doFetch(track, true);
  }, [track, doFetch]);

  // Sync lyrics from other window via storage event
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (track?.id && e.key === `lyrics_${track.id}` && e.newValue) {
        try {
          const parsed = JSON.parse(e.newValue) as LyricsInfo;
          setLyrics(parsed);
          setError(null);
        } catch {}
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
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

  return { lyrics, currentLineIndex, loading, error, refetchLyrics };
}
