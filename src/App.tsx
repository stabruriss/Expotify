import { useAuth } from "./hooks/useAuth";
import { useTrack } from "./hooks/useTrack";
import "./App.css";

function App() {
  const { authStatus, loading: authLoading, loginSpotify, loginOpenai, logoutSpotify, logoutOpenai } = useAuth();
  const { track, loading: trackLoading, error: trackError } = useTrack({
    enabled: authStatus.spotify,
    pollInterval: 3,
  });

  // Show auth screen if not logged in
  if (!authStatus.spotify) {
    return (
      <main className="container auth-screen">
        <h1>Expotify</h1>
        <p>Connect your accounts to get started</p>

        <div className="auth-buttons">
          <button
            onClick={loginSpotify}
            disabled={authLoading}
            className="auth-button spotify"
          >
            {authLoading ? "Loading..." : "Connect Spotify"}
          </button>

          {authStatus.spotify && !authStatus.openai && (
            <button
              onClick={loginOpenai}
              disabled={authLoading}
              className="auth-button openai"
            >
              Connect ChatGPT (Optional)
            </button>
          )}
        </div>
      </main>
    );
  }

  // Main player view
  return (
    <main className="container player-screen">
      {/* Header with auth status */}
      <header className="header">
        <h1>Expotify</h1>
        <div className="auth-status">
          <span className={`status-dot ${authStatus.spotify ? 'connected' : ''}`} />
          <span className={`status-dot ${authStatus.openai ? 'connected' : ''}`} />
        </div>
      </header>

      {/* Track display */}
      {trackLoading && !track && (
        <div className="loading">Loading...</div>
      )}

      {trackError && (
        <div className="error">{trackError}</div>
      )}

      {track ? (
        <div className="track-info">
          {/* Album art */}
          {track.album_art_url && (
            <img
              src={track.album_art_url}
              alt={track.album}
              className="album-art"
            />
          )}

          {/* Track details */}
          <div className="track-details">
            <h2 className="track-name">{track.name}</h2>
            <p className="track-artist">{track.artist}</p>
            <p className="track-album">{track.album}</p>
          </div>

          {/* Playing indicator */}
          <div className={`playing-indicator ${track.is_playing ? 'playing' : 'paused'}`}>
            {track.is_playing ? '▶ Playing' : '⏸ Paused'}
          </div>

          {/* AI Description */}
          {track.ai_description && (
            <div className="ai-description">
              <p>{track.ai_description}</p>
            </div>
          )}

          {!track.ai_description && !authStatus.openai && (
            <div className="ai-prompt">
              <button onClick={loginOpenai} className="connect-ai-btn">
                Connect ChatGPT to see AI insights
              </button>
            </div>
          )}
        </div>
      ) : (
        <div className="no-track">
          <p>No track playing</p>
          <p className="hint">Play something on Spotify to see it here</p>
        </div>
      )}

      {/* Footer with logout */}
      <footer className="footer">
        <button onClick={logoutSpotify} className="logout-btn">
          Disconnect Spotify
        </button>
        {authStatus.openai && (
          <button onClick={logoutOpenai} className="logout-btn">
            Disconnect ChatGPT
          </button>
        )}
      </footer>
    </main>
  );
}

export default App;
