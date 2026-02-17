use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use super::keychain::KeychainStorage;

const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_PORT: u16 = 1455;
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
    #[allow(dead_code)]
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
        // Load token synchronously at construction time
        // so auth status is available immediately when the frontend queries it.
        let initial_token = match KeychainStorage::get::<OpenAIToken>(KEYCHAIN_KEY) {
            Ok(Some(token)) => {
                log::info!("Loaded stored OpenAI token");
                Some(token)
            }
            Ok(None) => None,
            Err(e) => {
                log::warn!("Failed to load stored OpenAI token: {}", e);
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

    /// Synchronous check — safe to call from non-async setup code.
    pub fn has_stored_token(&self) -> bool {
        self.token.try_read().map(|g| g.is_some()).unwrap_or(false)
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
        (0..32)
            .map(|_| {
                let idx = rng.gen_range(0..62);
                let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                chars[idx] as char
            })
            .collect()
    }

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
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", "expotify");

        Ok(url.to_string())
    }

    /// Start a local HTTP server, wait for the OAuth callback, and exchange the code.
    /// This blocks until the callback is received or an error occurs.
    pub async fn wait_for_callback(&self) -> Result<()> {
        let expected_state = self
            .state
            .read()
            .await
            .clone()
            .context("No state found, call get_auth_url first")?;

        // Run the blocking TCP listener in a separate thread
        let (code, _received_state) =
            tokio::task::spawn_blocking(move || listen_for_callback(expected_state))
                .await
                .context("Callback listener task failed")??;

        self.exchange_code(&code).await
    }

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

    pub async fn get_access_token(&self) -> Result<String> {
        let token = self.token.read().await;
        if let Some(ref t) = *token {
            if !t.is_expired() {
                return Ok(t.access_token.clone());
            }
            let refresh_token = t.refresh_token.clone();
            drop(token);
            self.refresh_token_internal(&refresh_token).await?;
            return Ok(self
                .token
                .read()
                .await
                .as_ref()
                .unwrap()
                .access_token
                .clone());
        }
        anyhow::bail!("Not authenticated")
    }

    pub async fn is_authenticated(&self) -> bool {
        self.token.read().await.is_some()
    }

    pub async fn logout(&self) -> Result<()> {
        KeychainStorage::delete(KEYCHAIN_KEY)?;
        *self.token.write().await = None;
        Ok(())
    }
}

impl Default for OpenAIAuth {
    fn default() -> Self {
        Self::new()
    }
}

/// Listen on localhost:1455 for the OAuth callback, return (code, state).
fn listen_for_callback(expected_state: String) -> Result<(String, String)> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", CALLBACK_PORT))
        .context("Failed to bind to callback port 1455. Is another instance running?")?;

    // Loop until we get the actual callback request (browser may send favicon, pre-connect, etc.)
    loop {
        let (stream, _) = listener.accept().context("Failed to accept connection")?;

        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
            // Empty or failed read — likely a pre-connect, skip
            continue;
        }

        let path = match request_line.split_whitespace().nth(1) {
            Some(p) => p.to_string(),
            None => continue,
        };

        // Ignore non-callback requests (favicon, etc.)
        if !path.starts_with("/auth/callback") {
            let not_found = "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n";
            let _ = (&stream).write_all(not_found.as_bytes());
            continue;
        }

        let url = Url::parse(&format!("http://localhost{}", path))
            .context("Failed to parse callback URL")?;

        let params: std::collections::HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let code = match params.get("code") {
            Some(c) => c.clone(),
            None => {
                let bad = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nMissing code";
                let _ = (&stream).write_all(bad.as_bytes());
                continue;
            }
        };

        let state = match params.get("state") {
            Some(s) => s.clone(),
            None => {
                let bad = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nMissing state";
                let _ = (&stream).write_all(bad.as_bytes());
                continue;
            }
        };

        if state != expected_state {
            let error_html = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h1>Authentication failed</h1><p>Invalid state parameter.</p></body></html>";
            let _ = (&stream).write_all(error_html.as_bytes());
            anyhow::bail!("State mismatch in OAuth callback");
        }

        let success_html = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body style=\"font-family:system-ui;display:flex;justify-content:center;align-items:center;height:100vh;background:#121212;color:#fff\"><div style=\"text-align:center\"><h1 style=\"color:#1DB954\">Connected!</h1><p>You can close this tab and return to Expotify.</p></div></body></html>";
        let _ = (&stream).write_all(success_html.as_bytes());

        return Ok((code, state));
    }
}
