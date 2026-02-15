import { useRef, useEffect } from "react";
import type { LyricsInfo, LyricsLine } from "../types";

interface LyricsDisplayProps {
  lyrics: LyricsInfo | null;
  currentLineIndex: number;
  loading: boolean;
  error: string | null;
}

const SOURCE_LABELS: Record<string, string> = {
  NetEase: "网易云音乐",
  Lrclib: "LRCLIB",
  PetitLyrics: "PetitLyrics",
};

function findTranslation(
  translations: LyricsLine[],
  timeMs: number
): string | null {
  if (translations.length === 0) return null;
  let best = translations[0];
  for (const t of translations) {
    if (Math.abs(t.time_ms - timeMs) < Math.abs(best.time_ms - timeMs)) {
      best = t;
    }
  }
  return Math.abs(best.time_ms - timeMs) < 500 ? best.text : null;
}

export function LyricsDisplay({
  lyrics,
  currentLineIndex,
  loading,
  error,
}: LyricsDisplayProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const activeLineRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to active line
  useEffect(() => {
    if (activeLineRef.current && containerRef.current) {
      const container = containerRef.current;
      const activeLine = activeLineRef.current;
      const containerHeight = container.clientHeight;
      const targetScrollTop = activeLine.offsetTop - containerHeight / 3;

      container.scrollTo({
        top: targetScrollTop,
        behavior: "smooth",
      });
    }
  }, [currentLineIndex]);

  if (loading) {
    return (
      <div className="lyrics-container">
        <div className="lyrics-status">Loading lyrics...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="lyrics-container">
        <div className="lyrics-status lyrics-error">Failed to load lyrics</div>
      </div>
    );
  }

  if (!lyrics) {
    return null;
  }

  // Instrumental track
  if (lyrics.is_instrumental) {
    return (
      <div className="lyrics-container">
        <div className="lyrics-instrumental">
          <p>纯音乐，无歌词</p>
        </div>
      </div>
    );
  }

  // Synced lyrics
  if (lyrics.synced_lines.length > 0) {
    return (
      <div className="lyrics-container" ref={containerRef}>
        <div className="lyrics-synced">
          {lyrics.synced_lines.map((line, index) => {
            const translation = findTranslation(
              lyrics.translation_lines,
              line.time_ms
            );
            return (
              <div
                key={`${line.time_ms}-${index}`}
                ref={index === currentLineIndex ? activeLineRef : null}
                className={`lyrics-line ${
                  index === currentLineIndex ? "active" : ""
                } ${index < currentLineIndex ? "past" : ""}`}
              >
                <span className="lyrics-text">
                  {line.text || "\u00A0"}
                </span>
                {translation && (
                  <span className="lyrics-translation">{translation}</span>
                )}
              </div>
            );
          })}
        </div>
        {lyrics.source !== "None" && (
          <div className="lyrics-source">
            {SOURCE_LABELS[lyrics.source] || lyrics.source}
          </div>
        )}
      </div>
    );
  }

  // Plain lyrics fallback
  if (lyrics.plain_lyrics) {
    return (
      <div className="lyrics-container" ref={containerRef}>
        <div className="lyrics-plain">
          <pre>{lyrics.plain_lyrics}</pre>
        </div>
        {lyrics.source !== "None" && (
          <div className="lyrics-source">
            {SOURCE_LABELS[lyrics.source] || lyrics.source}
          </div>
        )}
      </div>
    );
  }

  // No lyrics found
  return (
    <div className="lyrics-container">
      <div className="lyrics-status">暂无歌词</div>
    </div>
  );
}
