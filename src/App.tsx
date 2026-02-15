import { useState, useEffect, useCallback } from "react";
import Markdown from "react-markdown";
import { useAuth } from "./hooks/useAuth";
import { useTrack } from "./hooks/useTrack";
import { useLyrics } from "./hooks/useLyrics";
import { LyricsDisplay } from "./components/LyricsDisplay";
import { getSettings, updateSettings } from "./lib/tauri";
import type { Settings } from "./types";
import { AVAILABLE_MODELS, DEFAULT_AI_PROMPT } from "./types";
import "./App.css";

function App() {
  const { authStatus, loading: authLoading, loginOpenai, logoutOpenai } = useAuth();
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const {
    track,
    aiLoading,
    error: trackError,
    spotifyRunning,
    fetchAi,
  } = useTrack({ pollInterval: 3, autoAi: settings?.ai_auto ?? false });
  const { lyrics, currentLineIndex, loading: lyricsLoading, error: lyricsError } = useLyrics({ track });
  const [draftModel, setDraftModel] = useState("");
  const [draftPrompt, setDraftPrompt] = useState("");
  const [draftWebSearch, setDraftWebSearch] = useState(false);
  const [saving, setSaving] = useState(false);

  const loadSettings = useCallback(async () => {
    try {
      const s = await getSettings();
      setSettings(s);
      setDraftModel(s.ai_model);
      setDraftPrompt(s.ai_prompt);
      setDraftWebSearch(s.ai_web_search);
    } catch (e) {
      console.error("Failed to load settings", e);
    }
  }, []);

  useEffect(() => {
    loadSettings();
  }, [loadSettings]);

  const openSettings = () => {
    if (settings) {
      setDraftModel(settings.ai_model);
      setDraftPrompt(settings.ai_prompt);
      setDraftWebSearch(settings.ai_web_search);
    }
    setShowSettings(true);
  };

  const saveSettings = async () => {
    if (!settings) return;
    setSaving(true);
    try {
      const updated = { ...settings, ai_model: draftModel, ai_prompt: draftPrompt, ai_web_search: draftWebSearch };
      await updateSettings(updated);
      setSettings(updated);
      setShowSettings(false);
    } catch (e) {
      console.error("Failed to save settings", e);
    } finally {
      setSaving(false);
    }
  };

  const resetPrompt = () => {
    setDraftPrompt(DEFAULT_AI_PROMPT);
  };

  const toggleAutoAi = async () => {
    if (!settings) return;
    const updated = { ...settings, ai_auto: !settings.ai_auto };
    try {
      await updateSettings(updated);
      setSettings(updated);
    } catch (e) {
      console.error("Failed to toggle auto AI", e);
    }
  };

  return (
    <main className="container player-screen">
      <header className="header">
        <h1>Expotify</h1>
        <div className="header-right">
          <button className="settings-btn" onClick={openSettings} title="Settings">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </button>
          <div className="auth-status">
            <span
              className={`status-dot ${spotifyRunning ? "connected" : ""}`}
              title="Spotify"
            />
            <span
              className={`status-dot ${authStatus.openai ? "connected" : ""}`}
              title="OpenAI"
            />
          </div>
        </div>
      </header>

      {!spotifyRunning && (
        <div className="no-track">
          <p>Spotify is not running</p>
          <p className="hint">
            Open the Spotify desktop app to see your music here
          </p>
        </div>
      )}

      {/* removed global loading — track info now shows instantly */}

      {trackError && <div className="error">{trackError}</div>}

      {spotifyRunning && track ? (
        <div className="track-info">
          {track.album_art_url && (
            <img
              src={track.album_art_url}
              alt={track.album}
              className="album-art"
            />
          )}

          <div className="track-details">
            <h2 className="track-name">{track.name}</h2>
            <p className="track-artist">{track.artist}</p>
            <p className="track-album">{track.album}</p>
          </div>

          <div
            className={`playing-indicator ${track.is_playing ? "playing" : "paused"}`}
          >
            {track.is_playing ? "▶ Playing" : "⏸ Paused"}
          </div>

          {track.ai_description ? (
            <>
              <div className="ai-description">
                <Markdown>{track.ai_description}</Markdown>
                {track.ai_used_web_search && (
                  <span className="ai-source-badge">🌐 Web</span>
                )}
              </div>
              {authStatus.openai && (
                <div className="ai-controls">
                  <button onClick={fetchAi} disabled={aiLoading} className="ai-generate-btn">
                    {aiLoading ? "Generating..." : "Regenerate"}
                  </button>
                  <button
                    className={`auto-toggle ${settings?.ai_auto ? "active" : ""}`}
                    onClick={toggleAutoAi}
                  >
                    Auto
                  </button>
                </div>
              )}
            </>
          ) : aiLoading ? (
            <div className="ai-description ai-loading">
              <p>AI 正在生成介绍...</p>
            </div>
          ) : authStatus.openai ? (
            <div className="ai-controls">
              <button onClick={fetchAi} className="ai-generate-btn">
                AI Insight
              </button>
              <button
                className={`auto-toggle ${settings?.ai_auto ? "active" : ""}`}
                onClick={toggleAutoAi}
              >
                Auto
              </button>
            </div>
          ) : (
            <button onClick={loginOpenai} disabled={authLoading} className="connect-ai-btn">
              {authLoading ? "Connecting..." : "Connect ChatGPT to see AI insights"}
            </button>
          )}

          {/* Lyrics */}
          <LyricsDisplay
            lyrics={lyrics}
            currentLineIndex={currentLineIndex}
            loading={lyricsLoading}
            error={lyricsError}
          />
        </div>
      ) : spotifyRunning ? (
        <div className="no-track">
          <p>No track playing</p>
          <p className="hint">Play something on Spotify to see it here</p>
        </div>
      ) : null}

      <footer className="footer">
        {authStatus.openai ? (
          <button onClick={logoutOpenai} className="logout-btn">
            Disconnect ChatGPT
          </button>
        ) : (
          <button onClick={loginOpenai} disabled={authLoading} className="logout-btn">
            {authLoading ? "Connecting..." : "Connect ChatGPT"}
          </button>
        )}
      </footer>

      {/* Settings Popup */}
      {showSettings && (
        <div className="popup-overlay" onClick={() => setShowSettings(false)}>
          <div className="popup" onClick={(e) => e.stopPropagation()}>
            <div className="popup-header">
              <h3>Settings</h3>
              <button className="popup-close" onClick={() => setShowSettings(false)}>
                &times;
              </button>
            </div>

            <div className="popup-body">
              <label className="field-label">Model</label>
              <select
                className="field-select"
                value={draftModel}
                onChange={(e) => setDraftModel(e.target.value)}
              >
                {AVAILABLE_MODELS.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name} — {m.desc}
                  </option>
                ))}
              </select>

              <label className="field-toggle">
                <input
                  type="checkbox"
                  checked={draftWebSearch}
                  onChange={(e) => setDraftWebSearch(e.target.checked)}
                />
                <span>Web Search</span>
              </label>

              <div className="field-label-row">
                <label className="field-label">Prompt</label>
                <button className="reset-btn" onClick={resetPrompt}>Reset</button>
              </div>
              <textarea
                className="field-textarea"
                value={draftPrompt}
                onChange={(e) => setDraftPrompt(e.target.value)}
                rows={8}
              />
              <p className="field-hint">
                Variables: {"{name}"} {"{artist}"} {"{album}"}
              </p>
            </div>

            <div className="popup-footer">
              <button className="btn-secondary" onClick={() => setShowSettings(false)}>
                Cancel
              </button>
              <button className="btn-primary" onClick={saveSettings} disabled={saving}>
                {saving ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        </div>
      )}
    </main>
  );
}

export default App;
