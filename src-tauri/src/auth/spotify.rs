use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use super::keychain::KeychainStorage;

const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const REDIRECT_URI: &str = "http://localhost:8888/callback";
const KEYCHAIN_KEY: &str = "spotify_token";

// Scopes needed for reading playback state
const SCOPES: &[&str] = &["user-read-currently-playing", "user-read-playback-state"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl SpotifyToken {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at - Duration::minutes(5)
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
    refresh_token: Option<String>,
    scope: String,
}

pub struct SpotifyAuth {
    client_id: String,
    client: Client,
    token: Arc<RwLock<Option<SpotifyToken>>>,
    pkce_verifier: Arc<RwLock<Option<String>>>,
}

impl SpotifyAuth {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            client: Client::new(),
            token: Arc::new(RwLock::new(None)),
            pkce_verifier: Arc::new(RwLock::new(None)),
        }
    }

    /// Load token from keychain on startup
    pub async fn load_stored_token(&self) -> Result<bool> {
        if let Some(token) = KeychainStorage::get::<SpotifyToken>(KEYCHAIN_KEY)? {
            if !token.is_expired() {
                *self.token.write().await = Some(token);
                return Ok(true);
            }
            // Token expired, try to refresh
            if let Ok(()) = self.refresh_token_internal(&token.refresh_token).await {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Generate PKCE code verifier and challenge
    fn generate_pkce() -> (String, String) {
        let mut rng = rand::thread_rng();
        let verifier: String = (0..64)
            .map(|_| {
                let idx = rng.gen_range(0..62);
                let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                chars[idx] as char
            })
            .collect();

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        (verifier, challenge)
    }

    /// Get the authorization URL to open in browser
    pub async fn get_auth_url(&self) -> Result<String> {
        let (verifier, challenge) = Self::generate_pkce();
        *self.pkce_verifier.write().await = Some(verifier);

        let mut url = Url::parse(SPOTIFY_AUTH_URL)?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge);

        Ok(url.to_string())
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code(&self, code: &str) -> Result<()> {
        let verifier = self
            .pkce_verifier
            .read()
            .await
            .clone()
            .context("No PKCE verifier found")?;

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", &self.client_id),
            ("code_verifier", &verifier),
        ];

        let response = self
            .client
            .post(SPOTIFY_TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = SpotifyToken {
            access_token: response.access_token,
            refresh_token: response.refresh_token.unwrap_or_default(),
            expires_at: Utc::now() + Duration::seconds(response.expires_in),
        };

        KeychainStorage::store(KEYCHAIN_KEY, &token)?;
        *self.token.write().await = Some(token);
        *self.pkce_verifier.write().await = None;

        Ok(())
    }

    /// Refresh the access token
    async fn refresh_token_internal(&self, refresh_token: &str) -> Result<()> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.client_id),
        ];

        let response = self
            .client
            .post(SPOTIFY_TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = SpotifyToken {
            access_token: response.access_token,
            refresh_token: response
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_string()),
            expires_at: Utc::now() + Duration::seconds(response.expires_in),
        };

        KeychainStorage::store(KEYCHAIN_KEY, &token)?;
        *self.token.write().await = Some(token);

        Ok(())
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_access_token(&self) -> Result<String> {
        let token = self.token.read().await;
        if let Some(ref t) = *token {
            if !t.is_expired() {
                return Ok(t.access_token.clone());
            }
            let refresh_token = t.refresh_token.clone();
            drop(token);
            self.refresh_token_internal(&refresh_token).await?;
            return Ok(self.token.read().await.as_ref().unwrap().access_token.clone());
        }
        anyhow::bail!("Not authenticated")
    }

    /// Check if authenticated
    pub async fn is_authenticated(&self) -> bool {
        self.token.read().await.is_some()
    }

    /// Logout - clear stored token
    pub async fn logout(&self) -> Result<()> {
        KeychainStorage::delete(KEYCHAIN_KEY)?;
        *self.token.write().await = None;
        Ok(())
    }

    /// Get the redirect URI for the OAuth callback server
    pub fn get_redirect_uri() -> &'static str {
        REDIRECT_URI
    }

    /// Get the callback port
    pub fn get_callback_port() -> u16 {
        8888
    }
}
