import { useState, useMemo, useEffect, useCallback, useRef, type PointerEvent, type MouseEvent } from "react";
import Markdown from "react-markdown";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize, LogicalPosition } from "@tauri-apps/api/dpi";
import { useTrack } from "../hooks/useTrack";
import { useLyrics } from "../hooks/useLyrics";
import { getAuthStatus, showMainWindow, saveOverlayGeometry, spotifyPlayPause, spotifyNextTrack, spotifyPreviousTrack } from "../lib/tauri";
import { useUpdateCheck } from "../hooks/useUpdateCheck";
import frameImg from "./assets/frame.png";
import "./overlay.css";

/* Try to import generated watercolor assets; fall back gracefully */
let aiPanelBgImg: string | undefined;
let aiStampImg: string | undefined;
try { aiPanelBgImg = new URL("./assets/ai-panel-bg.png", import.meta.url).href; } catch {}
try { aiStampImg = new URL("./assets/ai-stamp.png", import.meta.url).href; } catch {}

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
  const { track, aiLoading, aiError, regenCooldown, spotifyRunning, fetchAi } = useTrack({
    pollInterval: 3,
    autoAi: false,
  });
  const { lyrics, currentLineIndex, loading: lyricsLoading, refetchLyrics } = useLyrics({ track });

  const { updateAvailable, latestVersion, openRelease, dismiss } = useUpdateCheck();

  const [collapsed, setCollapsed] = useState(false);
  const collapsedRef = useRef(false);
  const expandedGeoRef = useRef({ width: 420, height: 268 });

  const [aiVisible, setAiVisible] = useState(false);
  const [cachedAi, setCachedAi] = useState<string | null>(null);
  const [isAuthenticated, setIsAuthenticated] = useState(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Save overlay geometry on move/resize (restore is handled by Rust before window shows)
  useEffect(() => {
    const win = getCurrentWindow();

    const geo = { x: 0, y: 0, width: 420, height: 268 };
    let geoReady = false;

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
    const initGeo = async () => {
      try {
        const [pos, size, sf] = await Promise.all([win.outerPosition(), win.outerSize(), win.scaleFactor()]);
        const w = size.width / sf;
        const h = size.height / sf;
        if (w < 200 || h < 120) {
          // Saved geometry was from a collapsed state — restore defaults
          await win.setMinSize(new LogicalSize(300, 180));
          await win.setResizable(true);
          await win.setSize(new LogicalSize(420, 268));
          geo.x = pos.x / sf;
          geo.y = pos.y / sf;
          geo.width = 420;
          geo.height = 268;
        } else {
          geo.x = pos.x / sf;
          geo.y = pos.y / sf;
          geo.width = w;
          geo.height = h;
        }
      } catch {}
      geoReady = true;
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
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
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
        .then((status) => setIsAuthenticated(status.openai))
        .catch(() => {});
    };
    checkAuth();
    const interval = setInterval(checkAuth, 10000);
    return () => clearInterval(interval);
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
      setCachedAi(stored);
    } else {
      setCachedAi(null);
    }
  }, [track?.id]);

  useEffect(() => {
    if (track?.ai_description && track.id) {
      localStorage.setItem(`ai_insight_${track.id}`, track.ai_description);
      setCachedAi(track.ai_description);
      setAiVisible(true);
    }
  }, [track?.ai_description]);

  // Sync AI insight from other window via storage event
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (track?.id && e.key === `ai_insight_${track.id}` && e.newValue) {
        setCachedAi(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [track?.id]);

  const displayedAi = track?.ai_description ?? cachedAi;

  const handleAiInsight = useCallback(async () => {
    if (!isAuthenticated) {
      showMainWindow();
      return;
    }
    if (aiVisible) {
      setAiVisible(false);
    } else if (displayedAi) {
      setAiVisible(true);
    } else {
      await fetchAi();
    }
  }, [aiVisible, displayedAi, fetchAi, isAuthenticated]);

  const handleRegenerate = useCallback(async () => {
    await fetchAi(true);
  }, [fetchAi]);

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

  const handleToggleCollapse = useCallback(async () => {
    const win = getCurrentWindow();
    const sf = await win.scaleFactor();

    if (!collapsed) {
      const size = await win.outerSize();
      expandedGeoRef.current = { width: size.width / sf, height: size.height / sf };
      // Mark collapsed BEFORE resizing so the resize handler skips saving 72x72
      collapsedRef.current = true;
      await win.setMinSize(new LogicalSize(72, 72));
      await win.setResizable(false);
      await win.setSize(new LogicalSize(72, 72));
    } else {
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
    }
    setCollapsed(!collapsed);
  }, [collapsed]);

  const visibleLines = useMemo(() => {
    if (!lyrics?.synced_lines?.length) return [];
    const lines = lyrics.synced_lines;
    const translations = lyrics.translation_lines;
    const center = currentLineIndex >= 0 ? currentLineIndex : 0;
    const result: Array<{
      text: string;
      translation?: string;
      offset: number;
      index: number;
    }> = [];
    const start = Math.max(0, center - HALF);
    const end = Math.min(lines.length - 1, center + HALF);
    for (let i = start; i <= end; i++) {
      const translation = translations?.find(
        (t) => Math.abs(t.time_ms - lines[i].time_ms) < 500
      );
      result.push({
        text: lines[i].text,
        translation: translation?.text,
        offset: i - center,
        index: i,
      });
    }
    return result;
  }, [lyrics, currentLineIndex]);

  /* Update notification bar */
  const updateBar = updateAvailable ? (
    <div className="overlay-update-bar" data-no-drag="true">
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
  const coverElement = track ? (
    <div className={`overlay-cover-wrapper ${track.is_playing ? "playing" : ""}`}>
      <div className="overlay-vinyl-disc" />
      {track.album_art_url ? (
        <img className="overlay-cover" src={track.album_art_url} alt="" draggable={false} />
      ) : (
        <div className="overlay-cover-placeholder" />
      )}
    </div>
  ) : null;

  /* Cover area: non-rotating wrapper with control buttons overlay */
  const coverArea = track ? (
    <div className="overlay-cover-area">
      {coverElement}
      {/* Top-left: collapse/expand */}
      <button
        className="overlay-cover-btn overlay-btn-tl"
        data-no-drag="true"
        onClick={handleToggleCollapse}
        title={collapsed ? "Expand" : "Collapse"}
      >
        <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="rgba(51,166,184,0.85)" strokeWidth="1.8" strokeLinecap="round">
          {collapsed ? <path d="M2 5L4 3L6 5" /> : <path d="M2 3L4 5L6 3" />}
        </svg>
      </button>
      {/* Top-right: play/pause */}
      <button
        className="overlay-cover-btn overlay-btn-tr"
        data-no-drag="true"
        onClick={() => { spotifyPlayPause().catch(() => {}); }}
        title={track.is_playing ? "Pause" : "Play"}
      >
        {track.is_playing ? (
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
        onClick={() => { spotifyPreviousTrack().catch(() => {}); }}
        title="Previous"
      >
        <svg width="7" height="7" viewBox="0 0 8 8" fill="rgba(51,166,184,0.85)">
          <polygon points="4.5,1.5 1.5,4 4.5,6.5" />
          <line x1="1.2" y1="1.5" x2="1.2" y2="6.5" stroke="rgba(51,166,184,0.85)" strokeWidth="1.2" />
        </svg>
      </button>
      {/* Bottom-right: next */}
      <button
        className="overlay-cover-btn overlay-btn-br"
        data-no-drag="true"
        onClick={() => { spotifyNextTrack().catch(() => {}); }}
        title="Next"
      >
        <svg width="7" height="7" viewBox="0 0 8 8" fill="rgba(51,166,184,0.85)">
          <polygon points="3.5,1.5 6.5,4 3.5,6.5" />
          <line x1="6.8" y1="1.5" x2="6.8" y2="6.5" stroke="rgba(51,166,184,0.85)" strokeWidth="1.2" />
        </svg>
      </button>
    </div>
  ) : null;

  if (collapsed && track) {
    return (
      <div className="overlay-frame overlay-collapsed" onMouseDown={handleMouseDown}>
        <div className="overlay-compact-content">
          {coverArea}
        </div>
      </div>
    );
  }

  if (!spotifyRunning) {
    return (
      <div className="overlay-frame" onMouseDown={handleMouseDown}>
        <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />
        <div className="overlay-content">
          {updateBar}
          <div className="overlay-not-playing">Spotify is not running</div>
        </div>
        {resizeHandles}
      </div>
    );
  }

  if (!track) {
    return (
      <div className="overlay-frame" onMouseDown={handleMouseDown}>
        <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />
        <div className="overlay-content">
          {updateBar}
          <div className="overlay-not-playing">No track playing</div>
        </div>
        {resizeHandles}
      </div>
    );
  }

  return (
    <div className="overlay-frame" onMouseDown={handleMouseDown}>
      <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />

      <div className="overlay-content">
        {updateBar}
        {/* Header: vinyl cover + song info + open button + AI stamp */}
        <div className="overlay-header">
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
              disabled={aiLoading && !displayedAi}
            >
              {aiStampImg ? (
                <img className="overlay-ai-stamp" src={aiStampImg} alt="AI" draggable={false} />
              ) : (
                "AI"
              )}
            </button>
            <span className={getLedClass(aiLoading, !!displayedAi)} />
          </div>
        </div>

        {/* Body: lyrics + AI overlay */}
        <div className="overlay-body">
          {/* Lyrics refresh button */}
          <button
            className="overlay-lyrics-refresh"
            data-no-drag="true"
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
          {/* Lyrics */}
          <div className="overlay-lyrics">
            {lyrics?.is_instrumental ? (
              <div className="overlay-no-lyrics">Instrumental</div>
            ) : visibleLines.length > 0 ? (
              visibleLines.map((line) => (
                <div
                  key={line.index}
                  className={`overlay-lyrics-line ${getLineClass(line.offset)}`}
                >
                  {line.text || "..."}
                  {line.translation && (
                    <div className="overlay-lyrics-translation">
                      {line.translation}
                    </div>
                  )}
                </div>
              ))
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

          {/* AI panel - full-width overlay on lyrics */}
          {aiVisible && displayedAi && (
            <div className="overlay-ai-section" data-no-drag="true">
              {aiPanelBgImg && (
                <div
                  className="overlay-ai-bg"
                  style={{ backgroundImage: `url(${aiPanelBgImg})` }}
                />
              )}
              <div className="overlay-ai-content" data-no-drag="true">
                <div className="overlay-ai-text">
                  <Markdown>{displayedAi}</Markdown>
                  {track.ai_used_web_search && (
                    <span className="overlay-ai-web-badge">web</span>
                  )}
                </div>
                <div className="overlay-ai-footer">
                  <button
                    className="overlay-ai-regen"
                    onClick={handleRegenerate}
                    disabled={aiLoading || regenCooldown}
                  >
                    {aiLoading ? "Generating..." : regenCooldown ? "Cooldown..." : "Regenerate"}
                  </button>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>

      {resizeHandles}
    </div>
  );
}
