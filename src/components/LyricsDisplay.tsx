import { useState, useRef, useEffect } from "react";
import type { LyricsInfo, LyricsLine } from "../types";

interface LyricsDisplayProps {
  lyrics: LyricsInfo | null;
  currentLineIndex: number;
  loading: boolean;
  error: string | null;
  onRefresh?: () => void;
}

const SOURCE_LABELS: Record<string, string> = {
  NetEase: "NetEase (网易云音乐)",
  QQMusic: "QQ Music (QQ音乐)",
  Kugou: "Kugou (酷狗音乐)",
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

function SourceBadge({ source }: { source: string }) {
  if (source === "None") return null;
  return (
    <div className="lyrics-source-bar">
      <span className="lyrics-source-label">source</span>
      <span className="lyrics-source-value">{SOURCE_LABELS[source] || source}</span>
    </div>
  );
}

function FetchLog({
  lyrics,
  loading,
  error,
  onRefresh,
}: {
  lyrics: LyricsInfo | null;
  loading: boolean;
  error: string | null;
  onRefresh?: () => void;
}) {
  const [expanded, setExpanded] = useState(false);

  const hasLog = (lyrics?.fetch_log && lyrics.fetch_log.length > 0) || error || loading;
  if (!hasLog) return null;

  return (
    <div className="lyrics-log-section">
      <button
        className="lyrics-log-toggle"
        onClick={() => setExpanded((v) => !v)}
      >
        <svg
          className={`lyrics-log-chevron ${expanded ? "open" : ""}`}
          width="10"
          height="10"
          viewBox="0 0 10 10"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M3 2l4 3-4 3" />
        </svg>
        <span>Fetch Log</span>
        {loading && <span className="lyrics-log-spinner" />}
      </button>
      {expanded && (
        <div className="lyrics-cli">
          {loading && !lyrics && (
            <div className="lyrics-cli-line">
              <span className="lyrics-cli-prompt">&gt;</span> Fetching lyrics...
            </div>
          )}
          {lyrics?.fetch_log?.map((entry, i) => {
            const isError = entry.toLowerCase().includes("error");
            const isNoMatch = entry.includes("no match") || entry.includes("exhausted");
            const cls = isError
              ? "lyrics-cli-err"
              : isNoMatch
                ? "lyrics-cli-warn"
                : "lyrics-cli-ok";
            return (
              <div key={i} className={`lyrics-cli-line ${cls}`}>
                <span className="lyrics-cli-prompt">&gt;</span> {entry}
              </div>
            );
          })}
          {error && (
            <div className="lyrics-cli-line lyrics-cli-err">
              <span className="lyrics-cli-prompt">&gt;</span> Error: {error}
            </div>
          )}
          {onRefresh && (
            <button className="lyrics-refresh-btn" onClick={onRefresh} disabled={loading}>
              {loading ? "Fetching..." : "Retry"}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function LyricsDisplay({
  lyrics,
  currentLineIndex,
  loading,
  error,
  onRefresh,
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

  // Nothing at all yet
  if (!lyrics && !loading && !error) {
    return null;
  }

  // Determine lyrics content
  let lyricsContent: React.ReactNode = null;

  if (lyrics) {
    if (lyrics.is_instrumental) {
      lyricsContent = (
        <div className="lyrics-instrumental">
          <p>Instrumental</p>
        </div>
      );
    } else if (lyrics.synced_lines.length > 0) {
      lyricsContent = (
        <div className="lyrics-synced" ref={containerRef}>
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
      );
    } else if (lyrics.plain_lyrics) {
      lyricsContent = (
        <div className="lyrics-plain" ref={containerRef}>
          <pre>{lyrics.plain_lyrics}</pre>
        </div>
      );
    } else {
      lyricsContent = (
        <div className="lyrics-empty">
          <span>No lyrics found</span>
          {onRefresh && (
            <button className="lyrics-refresh-btn" onClick={onRefresh} disabled={loading}>
              {loading ? "Fetching..." : "Retry"}
            </button>
          )}
        </div>
      );
    }
  } else if (loading) {
    lyricsContent = (
      <div className="lyrics-loading-placeholder">Fetching lyrics...</div>
    );
  } else if (error) {
    lyricsContent = (
      <div className="lyrics-empty">
        <span>Failed to fetch lyrics</span>
        {onRefresh && (
          <button className="lyrics-refresh-btn" onClick={onRefresh} disabled={loading}>
            Retry
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="lyrics-container">
      {lyrics && <SourceBadge source={lyrics.source} />}
      {lyricsContent}
      <FetchLog lyrics={lyrics} loading={loading} error={error} onRefresh={onRefresh} />
    </div>
  );
}
