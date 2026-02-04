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

// OpenAI Codex OAuth endpoints (same as used by OpenCode)
const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann"; // Public Codex client
const REDIRECT_URI: &str = "http://localhost:1455/callback";
const KEYCHAIN_KEY: &str = "openai_token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl OpenAIToken {
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
}

pub struct OpenAIAuth {
    client: Client,
    token: Arc<RwLock<Option<OpenAIToken>>>,
    pkce_verifier: Arc<RwLock<Option<String>>>,
    state: Arc<RwLock<Option<String>>>,
}

impl OpenAIAuth {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            token: Arc::new(RwLock::new(None)),
            pkce_verifier: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Load token from keychain on startup
    pub async fn load_stored_token(&self) -> Result<bool> {
        if let Some(token) = KeychainStorage::get::<OpenAIToken>(KEYCHAIN_KEY)? {
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

    /// Generate random state for CSRF protection
    fn generate_state() -> String {
        let mut rng = rand::thread_rng();
        (0..32)
            .map(|_| {
                let idx = rng.gen_range(0..62);
                let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                chars[idx] as char
            })
            .collect()
    }

    /// Get the authorization URL to open in browser
    pub async fn get_auth_url(&self) -> Result<String> {
        let (verifier, challenge) = Self::generate_pkce();
        let state = Self::generate_state();

        *self.pkce_verifier.write().await = Some(verifier);
        *self.state.write().await = Some(state.clone());

        let mut url = Url::parse(OPENAI_AUTH_URL)?;
        url.query_pairs_mut()
            .append_pair("client_id", OPENAI_CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", "openid profile email offline_access")
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge)
            .append_pair("state", &state)
            .append_pair("audience", "https://api.openai.com/v1");

        Ok(url.to_string())
    }

    /// Validate the state parameter from callback
    pub async fn validate_state(&self, received_state: &str) -> bool {
        let stored_state = self.state.read().await;
        stored_state.as_ref().map_or(false, |s| s == received_state)
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
            ("client_id", OPENAI_CLIENT_ID),
            ("code_verifier", &verifier),
        ];

        let response = self
            .client
            .post(OPENAI_TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = OpenAIToken {
            access_token: response.access_token,
            refresh_token: response.refresh_token.unwrap_or_default(),
            expires_at: Utc::now() + Duration::seconds(response.expires_in),
        };

        KeychainStorage::store(KEYCHAIN_KEY, &token)?;
        *self.token.write().await = Some(token);
        *self.pkce_verifier.write().await = None;
        *self.state.write().await = None;

        Ok(())
    }

    /// Refresh the access token
    async fn refresh_token_internal(&self, refresh_token: &str) -> Result<()> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENAI_CLIENT_ID),
        ];

        let response = self
            .client
            .post(OPENAI_TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = OpenAIToken {
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

    /// Get the callback port
    pub fn get_callback_port() -> u16 {
        1455
    }
}

impl Default for OpenAIAuth {
    fn default() -> Self {
        Self::new()
    }
}
