import { useState, useMemo, useEffect, useCallback, type MouseEvent } from "react";
import Markdown from "react-markdown";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTrack } from "../hooks/useTrack";
import { useLyrics } from "../hooks/useLyrics";
import frameImg from "./assets/frame.png";
import "./overlay.css";

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
  const { track, aiLoading, spotifyRunning, fetchAi } = useTrack({
    pollInterval: 3,
    autoAi: false,
  });
  const { lyrics, currentLineIndex } = useLyrics({ track });

  const [aiVisible, setAiVisible] = useState(false);
  const [cachedAi, setCachedAi] = useState<string | null>(null);

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

  const displayedAi = track?.ai_description ?? cachedAi;

  const handleAiInsight = useCallback(async () => {
    if (aiVisible) {
      setAiVisible(false);
    } else if (displayedAi) {
      setAiVisible(true);
    } else {
      await fetchAi();
    }
  }, [aiVisible, displayedAi, fetchAi]);

  const handleRegenerate = useCallback(async () => {
    await fetchAi();
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

  if (!spotifyRunning) {
    return (
      <div className="overlay-frame" onMouseDown={handleMouseDown}>
        <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />
        <div className="overlay-content">
          <div className="overlay-not-playing">Spotify is not running</div>
        </div>
      </div>
    );
  }

  if (!track) {
    return (
      <div className="overlay-frame" onMouseDown={handleMouseDown}>
        <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />
        <div className="overlay-content">
          <div className="overlay-not-playing">No track playing</div>
        </div>
      </div>
    );
  }

  return (
    <div className="overlay-frame" onMouseDown={handleMouseDown}>
      <div className="overlay-brush-frame" style={{ backgroundImage: `url(${frameImg})` }} />

      <div className="overlay-content">
        {/* Header: cover + song info + AI */}
        <div className="overlay-header">
          {track.album_art_url ? (
            <img className="overlay-cover" src={track.album_art_url} alt="" draggable={false} />
          ) : (
            <div className="overlay-cover overlay-cover-placeholder" />
          )}
          <div className="overlay-meta">
            <div className="overlay-track-name">{track.name}</div>
            <div className="overlay-track-artist">{track.artist}</div>
          </div>
          <div className="overlay-ai-toggle" data-no-drag="true">
            <button
              className="overlay-ai-btn"
              onClick={handleAiInsight}
              disabled={aiLoading && !displayedAi}
            >
              AI
            </button>
            <span className={getLedClass(aiLoading, !!displayedAi)} />
          </div>
        </div>

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
                  <div key={i} className="overlay-lyrics-line near">{line}</div>
                ))}
            </div>
          ) : (
            <div className="overlay-no-lyrics">No lyrics available</div>
          )}
        </div>

        {/* AI panel */}
        {aiVisible && displayedAi && (
          <div className="overlay-ai-section" data-no-drag="true">
            <div className="overlay-ai-content" data-no-drag="true">
              <div className="overlay-ai-text">
                <Markdown>{displayedAi}</Markdown>
              </div>
              <div className="overlay-ai-footer">
                <button
                  className="overlay-ai-regen"
                  onClick={handleRegenerate}
                  disabled={aiLoading}
                >
                  {aiLoading ? "Generating..." : "Regenerate"}
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
