import { useState, useEffect, useCallback, useMemo } from "react";
import Markdown from "react-markdown";
import { useAuth } from "./hooks/useAuth";
import { useTrack } from "./hooks/useTrack";
import { useLyrics } from "./hooks/useLyrics";
import { LyricsDisplay } from "./components/LyricsDisplay";
import { ModelPicker } from "./components/ModelPicker";
import { useModelCatalog } from "./hooks/useModelCatalog";
import { canSaveSelection } from "./lib/models";
import { getSettings, updateSettings } from "./lib/tauri";
import type { ModelSelection, Settings } from "./types";
import { DEFAULT_MODEL, DEFAULT_AI_PROMPT, DEFAULT_CHAT_PROMPT } from "./types";
import { RotateCcw, X } from "lucide-react";
import { useIMEComposition } from "./hooks/useIMEComposition";
import "./App.css";

function App() {
  const { onCompositionEnd: imeCompositionEnd, isIMEEnter } = useIMEComposition();
  const {
    authStatus,
    loading: authLoading,
    spotifyLoading,
    anthropicPending,
    error: authError,
    loginOpenai,
    logoutOpenai,
    startAnthropicLogin,
    cancelAnthropicLogin,
    logoutAnthropic,
    loginSpotify,
    connectSpotify,
    disconnectSpotify,
  } = useAuth();
  const [spDcInput, setSpDcInput] = useState("");
  const [spDcError, setSpDcError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const {
    track,
    aiLoading,
    aiError,
    regenCooldown,
    error: trackError,
    spotifyRunning,
    fetchAi,
  } = useTrack({ pollInterval: 3, autoAi: settings?.ai_auto ?? false });
  const { lyrics, currentLineIndex, loading: lyricsLoading, error: lyricsError, refetchLyrics } = useLyrics({ track });
  const [cachedAi, setCachedAi] = useState<{ trackId: string | null; text: string | null }>({
    trackId: null,
    text: null,
  });
  const [draftModel, setDraftModel] = useState<ModelSelection>(DEFAULT_MODEL);
  const [draftPrompt, setDraftPrompt] = useState("");
  const [draftWebSearch, setDraftWebSearch] = useState(false);
  const [draftChatModel, setDraftChatModel] = useState<ModelSelection>(DEFAULT_MODEL);
  const [draftChatPrompt, setDraftChatPrompt] = useState("");
  const [draftMemories, setDraftMemories] = useState<string[]>([]);
  const [newMemory, setNewMemory] = useState("");
  const [settingsTab, setSettingsTab] = useState<"insight" | "chat" | "memories">("insight");
  const [saving, setSaving] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const { catalogs, loading: modelsLoading, error: modelsError, refresh: refreshModels } = useModelCatalog(authStatus, showSettings);

  // AI is available if any provider is connected
  const aiConnected = authStatus.openai || authStatus.anthropic;

  // Read AI insight from localStorage cache when track changes
  useEffect(() => {
    if (track?.id) {
      const stored = localStorage.getItem(`ai_insight_${track.id}`);
      setCachedAi({ trackId: track.id, text: stored });
    } else {
      setCachedAi({ trackId: null, text: null });
    }
  }, [track?.id]);

  // Write AI insight to localStorage when it arrives
  useEffect(() => {
    if (track?.ai_description && track.id) {
      localStorage.setItem(`ai_insight_${track.id}`, track.ai_description);
      setCachedAi({ trackId: track.id, text: track.ai_description });
    }
  }, [track?.ai_description, track?.id]);

  // Sync AI insight from other window via storage event
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (track?.id && e.key === `ai_insight_${track.id}`) {
        setCachedAi({ trackId: track.id, text: e.newValue });
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [track?.id]);

  const displayedAi = useMemo(
    () => track?.ai_description ?? (track?.id && cachedAi.trackId === track.id ? cachedAi.text : null),
    [track?.ai_description, track?.id, cachedAi]
  );

  const applySettingsToDrafts = useCallback((s: Settings) => {
    setDraftModel(s.ai_model);
    setDraftPrompt(s.ai_prompt);
    setDraftWebSearch(s.ai_web_search);
    setDraftChatModel(s.chat_model);
    setDraftChatPrompt(s.chat_prompt);
    setDraftMemories([...s.memories]);
  }, []);

  const loadSettings = useCallback(async (syncDrafts = false) => {
    try {
      const s = await getSettings();
      setSettings(s);
      if (syncDrafts) {
        applySettingsToDrafts(s);
      }
      // Sync to localStorage for overlay window
      localStorage.setItem("expotify_settings_ai_auto", String(s.ai_auto));
      localStorage.setItem("expotify_settings_tts_volume", String(s.tts_volume));
    } catch (e) {
      console.error("Failed to load settings", e);
      setSettingsError(String(e));
    }
  }, [applySettingsToDrafts]);

  useEffect(() => {
    loadSettings(true);
  }, [loadSettings]);

  useEffect(() => {
    void loadSettings(false);
  }, [authStatus.openai, authStatus.anthropic, loadSettings]);

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== "expotify_settings_memories_updated_at") return;
      loadSettings(false);
      if (!showSettings) return;
      getSettings()
        .then((s) => {
          setSettings(s);
          setDraftMemories([...s.memories]);
        })
        .catch((err) => {
          console.error("Failed to sync memories", err);
        });
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [loadSettings, showSettings]);

  const openSettings = async () => {
    setSettingsError(null);
    try {
      await loadSettings(true);
    } catch {}
    setSettingsTab("insight");
    setNewMemory("");
    setShowSettings(true);
  };

  const saveSettings = async () => {
    if (!settings) return;
    if (!canSaveSelection(draftModel, settings.ai_model, catalogs, authStatus)
        || !canSaveSelection(draftChatModel, settings.chat_model, catalogs, authStatus)) {
      setSettingsError("Selected model is unavailable. Refresh the model list or choose another model.");
      return;
    }
    setSettingsError(null);
    setSaving(true);
    try {
      const updated = {
        ...settings,
        ai_model: draftModel,
        ai_prompt: draftPrompt,
        ai_web_search: draftWebSearch,
        chat_model: draftChatModel,
        chat_prompt: draftChatPrompt,
        memories: draftMemories,
      };
      await updateSettings(updated);
      setSettings(updated);
      setShowSettings(false);
    } catch (e) {
      console.error("Failed to save settings", e);
      setSettingsError(String(e));
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
      localStorage.setItem("expotify_settings_ai_auto", String(updated.ai_auto));
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
              title="Spotify App"
            />
            <span
              className={`status-dot ${authStatus.openai ? "connected" : ""}`}
              title="ChatGPT"
            />
            <span
              className={`status-dot ${authStatus.anthropic ? "connected" : ""}`}
              title="Claude"
            />
            <span
              className={`status-dot ${authStatus.spotify ? "connected" : ""}`}
              title="Spotify Auth"
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

      {trackError && <div className="error">{trackError}</div>}

      {spotifyRunning && track ? (
        <div className="track-info">
          {/* Horizontal header: cover + meta + play status */}
          <div className="track-header">
            {track.album_art_url && (
              <img
                src={track.album_art_url}
                alt={track.album}
                className="album-art"
              />
            )}
            <div className="track-meta">
              <h2 className="track-name">{track.name}</h2>
              <p className="track-artist">{track.artist}</p>
              <p className="track-album">{track.album}</p>
            </div>
            <div
              className={`playing-badge ${track.is_playing ? "playing" : "paused"}`}
            >
              {track.is_playing ? "\u25B6" : "\u23F8"}
            </div>
          </div>

          {/* AI section */}
          {aiError && <p className="ai-fetch-error">{aiError}</p>}
          {displayedAi ? (
            <>
              <div className="ai-description">
                <Markdown>{displayedAi}</Markdown>
                {track.ai_used_web_search && (
                  <span className="ai-source-badge">Web</span>
                )}
              </div>
              {aiConnected && (
                <div className="ai-controls">
                  <button onClick={() => fetchAi({ force: true, source: "manual" })} disabled={aiLoading || regenCooldown} className="ai-btn-skeu">
                    {aiLoading ? "Generating..." : regenCooldown ? "Cooldown..." : "Regenerate"}
                  </button>
                  <div className="auto-mode">
                    <label className="toggle-switch">
                      <input type="checkbox" checked={settings?.ai_auto ?? false} onChange={toggleAutoAi} />
                      <span className="toggle-track" />
                    </label>
                    <div className="auto-mode-info">
                      <span className="auto-mode-label">Auto mode</span>
                      <span className="auto-mode-desc">Auto generate AI insights for new track</span>
                    </div>
                  </div>
                </div>
              )}
            </>
          ) : aiLoading ? (
            <div className="ai-description ai-loading">
              <p>Generating AI insights...</p>
            </div>
          ) : aiConnected ? (
            <div className="ai-controls">
              <button onClick={() => fetchAi({ source: "manual" })} className="ai-btn-skeu">
                AI Insight
              </button>
              <div className="auto-mode">
                <label className="toggle-switch">
                  <input type="checkbox" checked={settings?.ai_auto ?? false} onChange={toggleAutoAi} />
                  <span className="toggle-track" />
                </label>
                <div className="auto-mode-info">
                  <span className="auto-mode-label">Auto mode</span>
                  <span className="auto-mode-desc">Auto generate AI insights for new track</span>
                </div>
              </div>
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
            onRefresh={refetchLyrics}
          />
        </div>
      ) : spotifyRunning ? (
        <div className="no-track">
          <p>No track playing</p>
          <p className="hint">Play something on Spotify to see it here</p>
        </div>
      ) : null}

      {/* Connect services section — parallel CTA buttons for unconnected services */}
      {(!authStatus.openai || !authStatus.anthropic || !authStatus.spotify || anthropicPending) && (
        <div className="connect-section">
          <div className="connect-buttons">
            {!authStatus.openai && (
              <button className="btn-primary connect-btn" onClick={loginOpenai} disabled={authLoading}>
                {authLoading ? "Connecting..." : "Connect ChatGPT"}
              </button>
            )}
            {!authStatus.anthropic && !anthropicPending && (
              <button
                className="btn-primary connect-btn"
                onClick={startAnthropicLogin}
                disabled={authLoading}
              >
                {authLoading ? "Opening login..." : "Connect Claude Max/Pro"}
              </button>
            )}
            {!authStatus.spotify && (
              <button className="btn-primary connect-btn" onClick={loginSpotify} disabled={spotifyLoading}>
                {spotifyLoading ? "Connecting..." : "Connect Spotify"}
              </button>
            )}
          </div>
          {(authError || spDcError) && <p className="sp-dc-error">{authError || spDcError}</p>}
          {!authStatus.spotify && (
            <details className="sp-dc-help">
              <summary>Manual Spotify connection (advanced)</summary>
              <p className="sp-dc-help-text">If the login window doesn&apos;t work, paste your sp_dc cookie manually:</p>
              <div className="sp-dc-input-row">
                <input
                  type="password"
                  className="field-input"
                  placeholder="Paste sp_dc cookie value"
                  value={spDcInput}
                  onChange={(e) => { setSpDcInput(e.target.value); setSpDcError(null); }}
                />
                <button
                  className="btn-primary sp-dc-connect-btn"
                  disabled={!spDcInput.trim() || spotifyLoading}
                  onClick={async () => {
                    try {
                      setSpDcError(null);
                      await connectSpotify(spDcInput.trim());
                      setSpDcInput("");
                    } catch (err) {
                      setSpDcError(err instanceof Error ? err.message : String(err));
                    }
                  }}
                >
                  {spotifyLoading ? "..." : "Connect"}
                </button>
              </div>
              <ol>
                <li>Open <strong>open.spotify.com</strong> and sign in</li>
                <li>Open DevTools (F12) &rarr; Application &rarr; Cookies</li>
                <li>Find <code>sp_dc</code> and copy its value</li>
              </ol>
            </details>
          )}
          {anthropicPending && !authStatus.anthropic && (
            <div className="sp-dc-help">
              <p className="sp-dc-help-text" role="status">Waiting for Claude sign-in...</p>
              <div className="sp-dc-input-row">
                <button
                  className="logout-btn"
                  onClick={cancelAnthropicLogin}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Footer — only disconnect buttons for connected services */}
      <footer className="footer">
        {authStatus.openai && (
          <button onClick={logoutOpenai} className="logout-btn">
            Disconnect ChatGPT
          </button>
        )}
        {authStatus.anthropic && (
          <button onClick={logoutAnthropic} className="logout-btn">
            Disconnect Claude
          </button>
        )}
        {authStatus.spotify && (
          <button onClick={disconnectSpotify} className="logout-btn">
            Disconnect Spotify
          </button>
        )}
      </footer>

      {/* Settings Popup */}
      {showSettings && (
        <div className="popup-overlay" onClick={() => setShowSettings(false)}>
          <div className="popup" role="dialog" aria-modal="true" aria-label="Settings" onClick={(e) => e.stopPropagation()}>
            <div className="popup-header">
              <h3>Settings</h3>
              <button className="popup-close model-icon-button" aria-label="Close settings" title="Close" onClick={() => setShowSettings(false)}>
                <X size={18} />
              </button>
            </div>

            <div className="popup-tabs">
              <button className={`popup-tab${settingsTab === "insight" ? " active" : ""}`} onClick={() => setSettingsTab("insight")}>AI Insight</button>
              <button className={`popup-tab${settingsTab === "chat" ? " active" : ""}`} onClick={() => setSettingsTab("chat")}>Chat</button>
              <button className={`popup-tab${settingsTab === "memories" ? " active" : ""}`} onClick={() => setSettingsTab("memories")}>Memories</button>
            </div>

            <div className="popup-body">
              {settingsTab === "insight" && (
                <>
                  <ModelPicker value={draftModel} onChange={setDraftModel} catalogs={catalogs} auth={authStatus}
                    loading={modelsLoading} error={modelsError} onRefresh={() => void refreshModels()} />

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
                    <button className="model-icon-button" aria-label="Reset insight prompt" title="Reset prompt" onClick={resetPrompt}><RotateCcw size={14} /></button>
                  </div>
                  <textarea
                    className="field-textarea"
                    value={draftPrompt}
                    onChange={(e) => setDraftPrompt(e.target.value)}
                    rows={8}
                  />
                  <p className="field-hint">
                    Variables: {"{name}"} {"{artist}"} {"{album}"} {"{memories}"}
                  </p>
                </>
              )}

              {settingsTab === "chat" && (
                <>
                  <ModelPicker value={draftChatModel} onChange={setDraftChatModel} catalogs={catalogs} auth={authStatus}
                    loading={modelsLoading} error={modelsError} onRefresh={() => void refreshModels()} />

                  <div className="field-label-row">
                    <label className="field-label">Chat Prompt</label>
                    <button className="model-icon-button" aria-label="Reset chat prompt" title="Reset prompt" onClick={() => setDraftChatPrompt(DEFAULT_CHAT_PROMPT)}><RotateCcw size={14} /></button>
                  </div>
                  <textarea
                    className="field-textarea"
                    value={draftChatPrompt}
                    onChange={(e) => setDraftChatPrompt(e.target.value)}
                    rows={12}
                  />
                  <p className="field-hint">
                    Variables: {"{name}"} {"{artist}"} {"{album}"} {"{volume}"} {"{memories}"}
                  </p>
                </>
              )}

              {settingsTab === "memories" && (
                <>
                  {draftMemories.length === 0 ? (
                    <div className="memories-empty">No memories yet. The AI will save memories about your preferences as you chat.</div>
                  ) : (
                    <div className="memories-list">
                      {draftMemories.map((mem, i) => (
                        <div key={i} className="memory-item">
                          <span>{mem}</span>
                          <button
                            className="memory-delete-btn"
                            onClick={() => setDraftMemories(draftMemories.filter((_, j) => j !== i))}
                            title="Delete"
                          >
                            &times;
                          </button>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="memory-add-row">
                    <input
                      className="field-input"
                      type="text"
                      placeholder="Add a memory..."
                      value={newMemory}
                      onChange={(e) => setNewMemory(e.target.value)}
                      onCompositionEnd={imeCompositionEnd}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !isIMEEnter() && newMemory.trim()) {
                          setDraftMemories([...draftMemories, newMemory.trim()]);
                          setNewMemory("");
                        }
                      }}
                    />
                    <button
                      className="btn-primary"
                      disabled={!newMemory.trim()}
                      onClick={() => {
                        if (newMemory.trim()) {
                          setDraftMemories([...draftMemories, newMemory.trim()]);
                          setNewMemory("");
                        }
                      }}
                    >
                      Add
                    </button>
                  </div>
                </>
              )}
            </div>

            {settingsError && <p className="settings-error" role="alert">{settingsError}</p>}
            <div className="popup-footer">
              <button className="btn-secondary" onClick={() => setShowSettings(false)}>
                Cancel
              </button>
              <button className="btn-primary" onClick={saveSettings} disabled={saving || !settings}>
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
