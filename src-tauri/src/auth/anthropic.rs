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

const CLAUDE_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
const CLAUDE_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference";
const KEYCHAIN_KEY: &str = "anthropic_oauth_token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl AnthropicToken {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at - Duration::minutes(5)
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
}

pub struct AnthropicAuth {
    client: Client,
    token: Arc<RwLock<Option<AnthropicToken>>>,
    pkce_verifier: Arc<RwLock<Option<String>>>,
    state: Arc<RwLock<Option<String>>>,
}

impl AnthropicAuth {
    pub fn new() -> Self {
        let initial_token = match KeychainStorage::get::<AnthropicToken>(KEYCHAIN_KEY) {
            Ok(Some(token)) => {
                log::info!("[anthropic_auth] Loaded stored Claude OAuth token");
                Some(token)
            }
            Ok(None) => {
                log::info!("[anthropic_auth] No stored Claude OAuth token found");
                None
            }
            Err(e) => {
                log::warn!("[anthropic_auth] Failed to load stored token: {}", e);
                None
            }
        };

        Self {
            client: Client::new(),
            token: Arc::new(RwLock::new(initial_token)),
            pkce_verifier: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(None)),
        }
    }

    pub fn has_stored_token(&self) -> bool {
        self.token.try_read().map(|g| g.is_some()).unwrap_or(false)
    }

    pub async fn is_authenticated(&self) -> bool {
        self.token.read().await.is_some()
    }

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

    fn generate_state() -> String {
        let mut rng = rand::thread_rng();
        (0..64)
            .map(|_| format!("{:x}", rng.gen_range(0..16)))
            .collect()
    }

    pub async fn get_auth_url(&self) -> Result<String> {
        let (verifier, challenge) = Self::generate_pkce();
        let state = Self::generate_state();

        *self.pkce_verifier.write().await = Some(verifier);
        *self.state.write().await = Some(state.clone());

        let mut url = Url::parse(CLAUDE_AUTH_URL)?;
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", CLAUDE_CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", CLAUDE_REDIRECT_URI)
            .append_pair("scope", CLAUDE_SCOPES)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);

        Ok(url.to_string())
    }

    pub async fn clear_pending_oauth(&self) {
        *self.pkce_verifier.write().await = None;
        *self.state.write().await = None;
    }

    pub async fn exchange_code(&self, code: &str) -> Result<()> {
        let verifier = self
            .pkce_verifier
            .read()
            .await
            .clone()
            .context("No Claude OAuth verifier found")?;
        let state = self
            .state
            .read()
            .await
            .clone()
            .context("No Claude OAuth state found")?;

        let cleaned_code = code
            .split('#')
            .next()
            .unwrap_or(code)
            .split('&')
            .next()
            .unwrap_or(code)
            .trim()
            .to_string();

        let response = self
            .client
            .post(CLAUDE_TOKEN_URL)
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            )
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://claude.ai/")
            .header("Origin", "https://claude.ai")
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLAUDE_CLIENT_ID,
                "code": cleaned_code,
                "redirect_uri": CLAUDE_REDIRECT_URI,
                "code_verifier": verifier,
                "state": state,
            }))
            .send()
            .await?;

        let body = parse_token_response(response).await?;
        self.store_token(body, None).await?;
        self.clear_pending_oauth().await;
        Ok(())
    }

    async fn refresh_token_internal(&self, refresh_token: &str) -> Result<()> {
        let response = self
            .client
            .post(CLAUDE_TOKEN_URL)
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            )
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://claude.ai/")
            .header("Origin", "https://claude.ai")
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLAUDE_CLIENT_ID,
            }))
            .send()
            .await?;

        let body = parse_token_response(response).await?;
        self.store_token(body, Some(refresh_token)).await
    }

    async fn store_token(
        &self,
        response: TokenResponse,
        fallback_refresh_token: Option<&str>,
    ) -> Result<()> {
        let expires_in = response.expires_in.unwrap_or(60 * 60);
        let token = AnthropicToken {
            access_token: response.access_token,
            refresh_token: response
                .refresh_token
                .or_else(|| fallback_refresh_token.map(ToString::to_string))
                .unwrap_or_default(),
            expires_at: Utc::now() + Duration::seconds(expires_in),
        };

        KeychainStorage::store(KEYCHAIN_KEY, &token)?;
        *self.token.write().await = Some(token);
        Ok(())
    }

    pub async fn get_access_token(&self) -> Result<String> {
        let token_guard = self.token.read().await;
        if let Some(ref token) = *token_guard {
            if !token.is_expired() {
                return Ok(token.access_token.clone());
            }

            let refresh_token = token.refresh_token.clone();
            drop(token_guard);

            if refresh_token.is_empty() {
                anyhow::bail!("Claude OAuth token expired and no refresh token is available");
            }

            self.refresh_token_internal(&refresh_token).await?;
            return Ok(self
                .token
                .read()
                .await
                .as_ref()
                .context("Claude OAuth token missing after refresh")?
                .access_token
                .clone());
        }

        anyhow::bail!("Claude is not authenticated")
    }

    pub async fn logout(&self) -> Result<()> {
        self.clear_pending_oauth().await;
        KeychainStorage::delete(KEYCHAIN_KEY)?;
        *self.token.write().await = None;
        Ok(())
    }
}

impl Default for AnthropicAuth {
    fn default() -> Self {
        Self::new()
    }
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenResponse> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|json| {
                json.get("error_description")
                    .and_then(|v| v.as_str())
                    .or_else(|| json.get("error").and_then(|v| v.as_str()))
                    .map(ToString::to_string)
            })
            .unwrap_or(body);
        anyhow::bail!("Claude OAuth token request failed: {} - {}", status, detail);
    }

    serde_json::from_str(&body).context("Failed to parse Claude OAuth token response")
}
