use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::keychain::KeychainStorage;

const KEYCHAIN_KEY: &str = "spotify_sp_dc";
const TOTP_RAW_URL: &str =
    "https://gist.githubusercontent.com/sonic-liberation/22ed9c6ba463899e933427f7de1f0eef/raw";
const SERVER_TIME_URL: &str = "https://open.spotify.com/api/server-time";
const TOKEN_URL: &str = "https://open.spotify.com/api/token";

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSpDc {
    value: String,
}

#[derive(Debug, Clone)]
struct SpotifyToken {
    access_token: String,
    expires_at: DateTime<Utc>,
}

impl SpotifyToken {
    fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at - Duration::minutes(5)
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "accessTokenExpirationTimestampMs")]
    expiration_timestamp_ms: i64,
}

#[derive(Debug, Deserialize)]
struct ServerTimeResponse {
    #[serde(rename = "serverTime")]
    server_time: u64,
}

#[derive(Debug, Deserialize)]
struct TotpEntry {
    v: u32,
    s: String,
}

pub struct SpotifyAuth {
    client: Client,
    sp_dc: Arc<RwLock<Option<String>>>,
    token: Arc<RwLock<Option<SpotifyToken>>>,
    totp_cache: Arc<RwLock<Option<(String, u32)>>>,
}

impl SpotifyAuth {
    pub fn new() -> Self {
        let initial_sp_dc = match KeychainStorage::get::<StoredSpDc>(KEYCHAIN_KEY) {
            Ok(Some(stored)) => {
                log::info!("Loaded stored Spotify sp_dc cookie");
                Some(stored.value)
            }
            Ok(None) => None,
            Err(e) => {
                log::warn!("Failed to load stored sp_dc: {}", e);
                None
            }
        };

        let client = Client::builder()
            .use_rustls_tls()
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            sp_dc: Arc::new(RwLock::new(initial_sp_dc)),
            token: Arc::new(RwLock::new(None)),
            totp_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Synchronous check for setup code.
    pub fn has_sp_dc(&self) -> bool {
        self.sp_dc.try_read().map(|g| g.is_some()).unwrap_or(false)
    }

    pub async fn is_authenticated(&self) -> bool {
        self.sp_dc.read().await.is_some()
    }

    /// Store sp_dc cookie and immediately exchange for a token.
    pub async fn set_sp_dc(&self, value: &str) -> Result<()> {
        let stored = StoredSpDc {
            value: value.to_string(),
        };
        KeychainStorage::store(KEYCHAIN_KEY, &stored)?;
        *self.sp_dc.write().await = Some(value.to_string());

        // Immediately try to get a token to validate the cookie
        self.exchange_token().await?;

        Ok(())
    }

    /// Remove sp_dc and clear cached token.
    pub async fn remove_sp_dc(&self) -> Result<()> {
        KeychainStorage::delete(KEYCHAIN_KEY)?;
        *self.sp_dc.write().await = None;
        *self.token.write().await = None;
        Ok(())
    }

    /// Get a valid access token, refreshing if necessary.
    pub async fn get_access_token(&self) -> Result<String> {
        // Check if we have a valid cached token
        {
            let token = self.token.read().await;
            if let Some(ref t) = *token {
                if !t.is_expired() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        // Token is expired or missing, exchange again
        self.exchange_token().await?;

        let token = self.token.read().await;
        token
            .as_ref()
            .map(|t| t.access_token.clone())
            .context("Failed to obtain Spotify access token")
    }

    /// Clear cached access token so next request forces a refresh.
    pub async fn invalidate_token(&self) {
        *self.token.write().await = None;
    }

    /// Exchange sp_dc cookie + TOTP for an access token.
    async fn exchange_token(&self) -> Result<()> {
        let sp_dc = self
            .sp_dc
            .read()
            .await
            .clone()
            .context("No sp_dc cookie stored")?;

        log::info!(
            "[token] Starting token exchange (sp_dc len={})",
            sp_dc.len()
        );

        // Fetch TOTP secret and generate TOTP
        let (totp_secret, totp_ver) = self
            .fetch_totp_secret()
            .await
            .inspect_err(|e| log::error!("[token] Failed to fetch TOTP secret: {}", e))?;
        log::info!("[token] TOTP secret fetched (ver={})", totp_ver);
        let server_time = self
            .get_server_time()
            .await
            .inspect_err(|e| log::error!("[token] Failed to get server time: {}", e))?;
        log::info!("[token] Server time: {}", server_time);

        // Base32-decode the TOTP secret before using as HMAC key
        let decoded_secret = data_encoding::BASE32
            .decode(totp_secret.as_bytes())
            .context("Failed to base32-decode TOTP secret")?;
        let totp = Self::generate_totp(&decoded_secret, server_time);
        log::info!("[token] Generated TOTP: {}", totp);

        let server_time_str = server_time.to_string();
        let response = self
            .client
            .get(TOKEN_URL)
            .query(&[
                ("reason", "transport"),
                ("productType", "web-player"),
                ("totp", &totp),
                ("totpServer", &totp),
                ("totpVer", &totp_ver.to_string()),
                ("sTime", &server_time_str),
                ("cTime", &server_time_str),
            ])
            .header("Cookie", format!("sp_dc={}", sp_dc))
            .header("User-Agent", Self::random_user_agent())
            .send()
            .await
            .context("Failed to request Spotify token")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                // sp_dc is invalid or expired
                anyhow::bail!("Spotify sp_dc cookie is invalid or expired. Please re-enter it.");
            }
            anyhow::bail!("Spotify token exchange failed ({}): {}", status, body);
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .context("Failed to parse Spotify token response")?;

        let expires_at = DateTime::from_timestamp_millis(token_resp.expiration_timestamp_ms)
            .unwrap_or_else(|| Utc::now() + Duration::hours(1));

        log::info!("Spotify access token obtained, expires at {}", expires_at);

        *self.token.write().await = Some(SpotifyToken {
            access_token: token_resp.access_token,
            expires_at,
        });
        Ok(())
    }

    /// Fetch the TOTP secret from the GitHub Gist (uses raw URL, cached).
    async fn fetch_totp_secret(&self) -> Result<(String, u32)> {
        // Return cached value if available
        {
            let cache = self.totp_cache.read().await;
            if let Some(ref cached) = *cache {
                return Ok(cached.clone());
            }
        }

        // Fetch from raw gist URL (not rate-limited like API endpoint)
        let content = self
            .client
            .get(TOTP_RAW_URL)
            .header("User-Agent", Self::random_user_agent())
            .send()
            .await
            .context("Failed to fetch TOTP gist")?
            .text()
            .await
            .context("Failed to read TOTP gist body")?;

        log::info!(
            "[token] TOTP raw content (first 200 chars): {}",
            &content[..content.len().min(200)]
        );

        // Parse as JSON array and find entry with highest version
        let entries: Vec<TotpEntry> = serde_json::from_str(&content)
            .inspect_err(|e| log::error!("[token] JSON parse error: {}", e))
            .context("Failed to parse TOTP entries")?;

        let entry = entries
            .into_iter()
            .max_by_key(|e| e.v)
            .context("No TOTP entries found")?;

        let result = (entry.s, entry.v);
        *self.totp_cache.write().await = Some(result.clone());
        Ok(result)
    }

    /// Get Spotify server time for TOTP synchronization.
    async fn get_server_time(&self) -> Result<u64> {
        let resp: ServerTimeResponse = self
            .client
            .get(SERVER_TIME_URL)
            .send()
            .await
            .context("Failed to fetch Spotify server time")?
            .json()
            .await
            .context("Failed to parse server time")?;

        Ok(resp.server_time)
    }

    fn random_user_agent() -> String {
        let mut rng = rand::thread_rng();
        let osx_minor = rng.gen_range(11..=15);
        let osx_patch = rng.gen_range(4..=9);
        let webkit_major = rng.gen_range(530..=537);
        let webkit_minor = rng.gen_range(30..=37);
        let chrome_major = rng.gen_range(80..=125);
        let chrome_build = rng.gen_range(3000..=6000);
        let chrome_patch = rng.gen_range(60..=200);
        let safari_major = rng.gen_range(530..=537);
        let safari_minor = rng.gen_range(30..=36);
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_{osx_minor}_{osx_patch}) \
             AppleWebKit/{webkit_major}.{webkit_minor} (KHTML, like Gecko) \
             Chrome/{chrome_major}.0.{chrome_build}.{chrome_patch} \
             Safari/{safari_major}.{safari_minor}"
        )
    }

    /// Generate a 6-digit TOTP using HMAC-SHA1.
    fn generate_totp(secret: &[u8], time: u64) -> String {
        let counter = time / 30;
        let counter_bytes = counter.to_be_bytes();

        let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(&counter_bytes);
        let result = mac.finalize().into_bytes();

        let offset = (result[result.len() - 1] & 0x0f) as usize;
        let code = ((result[offset] as u32 & 0x7f) << 24
            | (result[offset + 1] as u32) << 16
            | (result[offset + 2] as u32) << 8
            | (result[offset + 3] as u32))
            % 1_000_000;

        format!("{:06}", code)
    }
}

impl Default for SpotifyAuth {
    fn default() -> Self {
        Self::new()
    }
}
