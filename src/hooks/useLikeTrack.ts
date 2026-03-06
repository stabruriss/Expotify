import { useState, useEffect, useCallback, useRef } from "react";
import { spotifyIsTrackLiked, spotifyLikeTrack, spotifyUnlikeTrack } from "../lib/tauri";

interface UseLikeTrackOptions {
  trackId: string | null;
  spotifyAuthenticated: boolean;
}

const POLL_INTERVAL_MS = 2000;
const RATE_LIMIT_BACKOFF_MS = 10000;

export function useLikeTrack({ trackId, spotifyAuthenticated }: UseLikeTrackOptions) {
  const [liked, setLiked] = useState(false);
  const [loading, setLoading] = useState(false);
  const [rateLimited, setRateLimited] = useState(false);
  const rateLimitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const activeTrackRef = useRef<string | null>(null);
  // Grace period: after a toggle, ignore poll results until this timestamp
  const graceUntilRef = useRef<number>(0);

  // Poll like status every 2 seconds
  useEffect(() => {
    if (pollTimerRef.current) {
      clearTimeout(pollTimerRef.current);
      pollTimerRef.current = null;
    }

    if (!trackId || !spotifyAuthenticated) {
      setLiked(false);
      activeTrackRef.current = null;
      return;
    }

    activeTrackRef.current = trackId;
    // Reset grace period on track change so initial status syncs immediately
    graceUntilRef.current = 0;

    const checkLiked = async () => {
      const currentTrack = activeTrackRef.current;
      if (!currentTrack) return;
      try {
        const nextLiked = await spotifyIsTrackLiked(currentTrack);
        // Only update state if track hasn't changed AND we're past the grace period
        if (activeTrackRef.current === currentTrack && Date.now() >= graceUntilRef.current) {
          setLiked(nextLiked);
        }
      } catch (e) {
        const errMsg = String(e);
        if (errMsg.includes("429") || errMsg.toLowerCase().includes("rate")) {
          // Show rate limited tooltip for 1 second
          setRateLimited(true);
          if (rateLimitTimerRef.current) clearTimeout(rateLimitTimerRef.current);
          rateLimitTimerRef.current = setTimeout(() => setRateLimited(false), 1000);
          // Back off polling for 10 seconds
          pollTimerRef.current = setTimeout(checkLiked, RATE_LIMIT_BACKOFF_MS);
          return;
        }
        console.error("Failed to fetch like status:", e);
      }
      // Schedule next poll
      if (activeTrackRef.current === currentTrack) {
        pollTimerRef.current = setTimeout(checkLiked, POLL_INTERVAL_MS);
      }
    };

    // Initial check immediately
    void checkLiked();

    return () => {
      if (pollTimerRef.current) {
        clearTimeout(pollTimerRef.current);
        pollTimerRef.current = null;
      }
      if (rateLimitTimerRef.current) {
        clearTimeout(rateLimitTimerRef.current);
        rateLimitTimerRef.current = null;
      }
    };
  }, [trackId, spotifyAuthenticated]);

  const toggleLike = useCallback(async () => {
    if (!trackId || !spotifyAuthenticated || loading) return;
    const wasLiked = liked;
    // Optimistic update: change UI immediately
    setLiked(!wasLiked);
    setLoading(true);
    // Suppress poll overwrites for 5 seconds to let Spotify propagate the change
    graceUntilRef.current = Date.now() + 5000;
    try {
      if (wasLiked) {
        await spotifyUnlikeTrack(trackId);
      } else {
        await spotifyLikeTrack(trackId);
      }
    } catch (e) {
      // Revert optimistic update on failure
      setLiked(wasLiked);
      graceUntilRef.current = 0; // Allow polling to correct state
      console.error("Failed to toggle like:", e);
    } finally {
      setLoading(false);
    }
  }, [trackId, spotifyAuthenticated, liked, loading]);

  const refreshLikeStatus = useCallback(async () => {
    if (!trackId || !spotifyAuthenticated) return;
    try {
      const nextLiked = await spotifyIsTrackLiked(trackId);
      setLiked(nextLiked);
    } catch (e) {
      console.error("Failed to refresh like status:", e);
    }
  }, [trackId, spotifyAuthenticated]);

  return { liked, loading, rateLimited, toggleLike, refreshLikeStatus };
}
