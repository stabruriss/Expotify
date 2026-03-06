use anyhow::{Context, Result};
use futures_util::StreamExt;
use rand::Rng;
use reqwest::{
    header::{HeaderMap, HeaderValue, ORIGIN, REFERER},
    Client, RequestBuilder, Response, StatusCode,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::auth::SpotifyAuth;

use super::types::{SearchResult, SpotifyDevice};

/// GraphQL pathfinder API — works with sp_dc tokens (unlike api.spotify.com/v1 which returns 429)
const PATHFINDER_API: &str = "https://api-partner.spotify.com/pathfinder/v1/query";

/// Spotify internal spclient API — for collection (liked songs) operations
const SPCLIENT_API: &str = "https://spclient.wg.spotify.com";
const CONNECT_STATE_API: &str = "https://guc-spclient.spotify.com/connect-state";
const DEALER_API: &str = "wss://guc3-dealer.spotify.com/";

const MAX_RETRY_ATTEMPTS: usize = 3;

// Persisted GraphQL query hashes (from Spotify web player / Spotifly)
const HASH_SEARCH_DESKTOP: &str =
    "60efc08b8017f382e73ba2e02ac03d3c3b209610de99da618f36252e457665dd";
const HASH_ADD_TO_LIBRARY: &str =
    "656c491c3f65d9d08d259be6632f4ef1931540ebcf766488ed17f76bb9156d15";
const HASH_REMOVE_FROM_LIBRARY: &str =
    "1103bfd4b9d80275950bff95ef6d41a02cec3357e8f7ecd8974528043739677c";
const HASH_FETCH_LIBRARY_TRACKS: &str =
    "8474ec383b530ce3e54611fca2d8e3da57ef5612877838b8dbf00bd9fc692dfb";

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

/// Cached liked status with expiration time.
struct LikedCacheEntry {
    liked: bool,
    expires_at: std::time::Instant,
}

pub struct SpotifyWebApi {
    client: Client,
    auth: Arc<SpotifyAuth>,
    /// Cache of track_id → liked status with TTL.
    /// Populated on first check per track, updated on like/unlike.
    liked_cache: RwLock<HashMap<String, LikedCacheEntry>>,
}

impl SpotifyWebApi {
    pub fn new(auth: Arc<SpotifyAuth>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert("app-platform", HeaderValue::from_static("WebPlayer"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://open.spotify.com"));
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://open.spotify.com/"),
        );

        let ua = random_user_agent();
        log::info!("[spotify_webapi] Using User-Agent: {}", ua);

        let client = Client::builder()
            .user_agent(&ua)
            .default_headers(headers)
            .use_rustls_tls()
            .build()
            .unwrap_or_else(|e| {
                log::warn!(
                    "Failed to build Spotify Web API client with HTTP/2: {}. Falling back.",
                    e
                );
                Client::builder()
                    .user_agent(&ua)
                    .default_headers(HeaderMap::new())
                    .build()
                    .unwrap_or_else(|_| Client::new())
            });

        Self {
            client,
            auth,
            liked_cache: RwLock::new(HashMap::new()),
        }
    }

    // ── Public API ──────────────────────────────────────────────────────

    /// Search for tracks on Spotify via GraphQL pathfinder.
    pub async fn search_tracks(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>> {
        let variables = serde_json::json!({
            "searchTerm": query,
            "offset": 0,
            "limit": limit,
            "numberOfTopResults": 5,
            "includeAudiobooks": false
        });
        let variables_str = variables.to_string();
        let extensions = serde_json::json!({
            "persistedQuery": { "version": 1, "sha256Hash": HASH_SEARCH_DESKTOP }
        });
        let extensions_str = extensions.to_string();

        let resp: Value = self
            .send_with_retry("search_tracks", |token| {
                self.client.get(PATHFINDER_API).bearer_auth(token).query(&[
                    ("operationName", "searchDesktop"),
                    ("variables", variables_str.as_str()),
                    ("extensions", extensions_str.as_str()),
                ])
            })
            .await
            .context("GraphQL searchDesktop failed")?
            .json()
            .await
            .context("Failed to parse GraphQL search response")?;

        // Log any GraphQL-level errors
        if let Some(errors) = resp.get("errors") {
            log::warn!("[spotify_webapi] GraphQL search errors: {}", errors);
        }

        let items = resp
            .pointer("/data/searchV2/tracksV2/items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let results: Vec<SearchResult> = items
            .iter()
            .filter_map(|item| Self::graphql_search_track_to_result(item))
            .collect();

        log::info!(
            "[spotify_webapi] Search '{}' returned {} results",
            query,
            results.len()
        );
        Ok(results)
    }

    /// Check if a track is in the user's liked songs.
    /// Uses a per-track cache with 30-second TTL: first call does a comprehensive
    /// GraphQL scan (limit=500), subsequent calls return the cached result.
    /// Cache is also updated immediately on like/unlike operations.
    pub async fn is_track_liked(&self, track_id: &str) -> Result<bool> {
        // Fast path: return cached result if not expired
        {
            let cache = self.liked_cache.read().await;
            if let Some(entry) = cache.get(track_id) {
                if entry.expires_at > std::time::Instant::now() {
                    return Ok(entry.liked);
                }
            }
        }

        // Cache miss or expired: scan library
        log::info!(
            "[spotify_webapi] is_track_liked: checking {} (cache miss/expired)",
            track_id
        );
        let liked = self.is_track_liked_graphql(track_id).await?;

        // Store in cache with 30-second TTL
        {
            let mut cache = self.liked_cache.write().await;
            cache.insert(
                track_id.to_string(),
                LikedCacheEntry {
                    liked,
                    expires_at: std::time::Instant::now() + Duration::from_secs(30),
                },
            );
        }

        Ok(liked)
    }

    /// Check liked status via GraphQL. Uses a high limit to cover the full library in one request.
    /// The API only returns as many items as exist, so this won't over-fetch.
    async fn is_track_liked_graphql(&self, track_id: &str) -> Result<bool> {
        let target_uri = format!("spotify:track:{}", track_id);
        let variables = serde_json::json!({
            "offset": 0,
            "limit": 5000
        });
        let variables_str = variables.to_string();
        let extensions = serde_json::json!({
            "persistedQuery": { "version": 1, "sha256Hash": HASH_FETCH_LIBRARY_TRACKS }
        });
        let extensions_str = extensions.to_string();

        let resp: Value = self
            .send_with_retry("is_track_liked_graphql", |token| {
                self.client.get(PATHFINDER_API).bearer_auth(token).query(&[
                    ("operationName", "fetchLibraryTracks"),
                    ("variables", variables_str.as_str()),
                    ("extensions", extensions_str.as_str()),
                ])
            })
            .await
            .context("GraphQL fetchLibraryTracks failed")?
            .json()
            .await
            .context("Failed to parse GraphQL library response")?;

        if let Some(errors) = resp.get("errors") {
            log::warn!("[spotify_webapi] GraphQL library errors: {}", errors);
        }

        // Try multiple possible response structures
        let items = resp
            .pointer("/data/me/libraryV3/items")
            .or_else(|| resp.pointer("/data/me/libraryV3/tracks/items"))
            .or_else(|| resp.pointer("/data/me/library/tracks/items"))
            .or_else(|| resp.pointer("/data/me/library/items"))
            .and_then(|v| v.as_array());

        if items.is_none() {
            let resp_str = serde_json::to_string_pretty(&resp).unwrap_or_default();
            log::warn!(
                "[spotify_webapi] is_track_liked_graphql: no items array. Response:\n{}",
                &resp_str[..resp_str.len().min(2000)]
            );
            return Ok(false);
        }

        if let Some(items) = items {
            for item in items {
                if let Some(result) = Self::graphql_library_track_to_result(item) {
                    if result.uri == target_uri {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    /// Add a track to liked songs via GraphQL.
    pub async fn like_track(&self, track_id: &str) -> Result<()> {
        self.like_track_graphql(track_id).await?;
        // Update cache with longer TTL (user just acted, trust the result)
        self.liked_cache.write().await.insert(
            track_id.to_string(),
            LikedCacheEntry {
                liked: true,
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
        Ok(())
    }

    /// Fallback: add to library via GraphQL mutation.
    async fn like_track_graphql(&self, track_id: &str) -> Result<()> {
        let uri = format!("spotify:track:{}", track_id);
        let variables = serde_json::json!({"uris": [uri]});
        let body = serde_json::json!({
            "variables": variables,
            "operationName": "addToLibrary",
            "extensions": {
                "persistedQuery": { "version": 1, "sha256Hash": HASH_ADD_TO_LIBRARY }
            }
        });

        let resp: Value = self
            .send_with_retry("like_track", |token| {
                self.client
                    .post(PATHFINDER_API)
                    .bearer_auth(token)
                    .json(&body)
            })
            .await
            .context("GraphQL addToLibrary failed")?
            .json()
            .await
            .context("Failed to parse addToLibrary response")?;

        if let Some(errors) = resp.get("errors") {
            log::warn!("[spotify_webapi] GraphQL addToLibrary errors: {}", errors);
            anyhow::bail!("GraphQL addToLibrary returned errors: {}", errors);
        }

        log::info!("[spotify_webapi] Liked track {}", track_id);
        Ok(())
    }

    /// Remove a track from liked songs via GraphQL.
    pub async fn unlike_track(&self, track_id: &str) -> Result<()> {
        self.unlike_track_graphql(track_id).await?;
        // Update cache with longer TTL (user just acted, trust the result)
        self.liked_cache.write().await.insert(
            track_id.to_string(),
            LikedCacheEntry {
                liked: false,
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
        Ok(())
    }

    /// Fallback: remove from library via GraphQL mutation.
    async fn unlike_track_graphql(&self, track_id: &str) -> Result<()> {
        let uri = format!("spotify:track:{}", track_id);
        let variables = serde_json::json!({"uris": [uri]});
        let body = serde_json::json!({
            "variables": variables,
            "operationName": "removeFromLibrary",
            "extensions": {
                "persistedQuery": { "version": 1, "sha256Hash": HASH_REMOVE_FROM_LIBRARY }
            }
        });

        let resp: Value = self
            .send_with_retry("unlike_track", |token| {
                self.client
                    .post(PATHFINDER_API)
                    .bearer_auth(token)
                    .json(&body)
            })
            .await
            .context("GraphQL removeFromLibrary failed")?
            .json()
            .await
            .context("Failed to parse removeFromLibrary response")?;

        if let Some(errors) = resp.get("errors") {
            log::warn!(
                "[spotify_webapi] GraphQL removeFromLibrary errors: {}",
                errors
            );
            anyhow::bail!("GraphQL removeFromLibrary returned errors: {}", errors);
        }

        log::info!("[spotify_webapi] Unliked track {}", track_id);
        Ok(())
    }

    /// Get a random liked track from the user's library.
    /// Tries spclient collection API first, falls back to GraphQL.
    pub async fn get_random_liked_track(&self) -> Result<SearchResult> {
        // Try spclient collection paging API
        let url = format!("{}/collection/v2/paging", SPCLIENT_API);

        // Step 1: Get total count
        let total_resp = self
            .send_with_retry("shuffle_spclient_total", |token| {
                self.client.get(&url).bearer_auth(token).query(&[
                    ("uri", "spotify:collection"),
                    ("offset", "0"),
                    ("limit", "1"),
                ])
            })
            .await;

        match total_resp {
            Ok(response) => {
                let text = response.text().await.unwrap_or_default();
                log::info!(
                    "[spotify_webapi] shuffle spclient paging response (first 1000 chars):\n{}",
                    &text[..text.len().min(1000)]
                );

                // Try to parse JSON
                if let Ok(json) = serde_json::from_str::<Value>(&text) {
                    let total = json
                        .get("totalLength")
                        .or_else(|| json.get("total"))
                        .or_else(|| json.get("totalCount"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    if total > 0 {
                        log::info!("[spotify_webapi] shuffle spclient: total = {}", total);

                        // Step 2: Get random track
                        let offset = rand::random::<u64>() % total;
                        let offset_str = offset.to_string();
                        let item_resp = self
                            .send_with_retry("shuffle_spclient_item", |token| {
                                self.client.get(&url).bearer_auth(token).query(&[
                                    ("uri", "spotify:collection"),
                                    ("offset", offset_str.as_str()),
                                    ("limit", "1"),
                                ])
                            })
                            .await?
                            .text()
                            .await?;

                        if let Ok(json) = serde_json::from_str::<Value>(&item_resp) {
                            if let Some(items) = json
                                .get("items")
                                .or_else(|| json.get("item"))
                                .and_then(|v| v.as_array())
                            {
                                if let Some(item) = items.first() {
                                    if let Some(result) =
                                        Self::spclient_collection_item_to_result(item)
                                    {
                                        return Ok(result);
                                    }
                                    log::warn!(
                                        "[spotify_webapi] shuffle spclient: failed to parse item: {}",
                                        serde_json::to_string(item).unwrap_or_default()
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "[spotify_webapi] spclient paging failed: {}. Trying GraphQL fallback.",
                    e
                );
            }
        }

        // Fallback: GraphQL
        self.get_random_liked_track_graphql().await
    }

    /// Fallback: get random liked track via GraphQL fetchLibraryTracks.
    async fn get_random_liked_track_graphql(&self) -> Result<SearchResult> {
        let extensions = serde_json::json!({
            "persistedQuery": { "version": 1, "sha256Hash": HASH_FETCH_LIBRARY_TRACKS }
        });
        let extensions_str = extensions.to_string();

        // First: get total count
        let vars1 = serde_json::json!({
            "offset": 0,
            "limit": 1
        });
        let vars1_str = vars1.to_string();

        let resp: Value = self
            .send_with_retry("get_random_liked_track_total", |token| {
                self.client.get(PATHFINDER_API).bearer_auth(token).query(&[
                    ("operationName", "fetchLibraryTracks"),
                    ("variables", vars1_str.as_str()),
                    ("extensions", extensions_str.as_str()),
                ])
            })
            .await?
            .json()
            .await
            .context("Failed to parse library total")?;

        if let Some(errors) = resp.get("errors") {
            log::warn!("[spotify_webapi] GraphQL shuffle errors: {}", errors);
        }

        let total = resp
            .pointer("/data/me/libraryV3/totalCount")
            .or_else(|| resp.pointer("/data/me/libraryV3/tracks/totalCount"))
            .or_else(|| resp.pointer("/data/me/library/tracks/totalCount"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        log::info!(
            "[spotify_webapi] shuffle graphql: total liked tracks = {}",
            total
        );

        if total == 0 {
            let resp_str = serde_json::to_string_pretty(&resp).unwrap_or_default();
            log::warn!(
                "[spotify_webapi] shuffle: no liked songs found. Response:\n{}",
                &resp_str[..resp_str.len().min(2000)]
            );
            anyhow::bail!("No liked songs found");
        }

        // Second: fetch one track at a random offset
        let offset = rand::random::<u64>() % total;
        let vars2 = serde_json::json!({
            "offset": offset,
            "limit": 1
        });
        let vars2_str = vars2.to_string();

        let resp: Value = self
            .send_with_retry("get_random_liked_track_item", |token| {
                self.client.get(PATHFINDER_API).bearer_auth(token).query(&[
                    ("operationName", "fetchLibraryTracks"),
                    ("variables", vars2_str.as_str()),
                    ("extensions", extensions_str.as_str()),
                ])
            })
            .await?
            .json()
            .await
            .context("Failed to parse random liked track")?;

        let items = resp
            .pointer("/data/me/libraryV3/items")
            .or_else(|| resp.pointer("/data/me/libraryV3/tracks/items"))
            .or_else(|| resp.pointer("/data/me/library/tracks/items"))
            .or_else(|| resp.pointer("/data/me/library/items"))
            .and_then(|v| v.as_array());

        if items.is_none() {
            let resp_str = serde_json::to_string_pretty(&resp).unwrap_or_default();
            log::warn!(
                "[spotify_webapi] shuffle: no items array. Response:\n{}",
                &resp_str[..resp_str.len().min(2000)]
            );
        }

        let items = items.context("No items array in library response")?;
        let track_data = items.first().context("No track at random offset")?;

        let result = Self::graphql_library_track_to_result(track_data);
        if result.is_none() {
            let track_str = serde_json::to_string_pretty(track_data).unwrap_or_default();
            log::warn!(
                "[spotify_webapi] shuffle: failed to parse track. Raw:\n{}",
                &track_str[..track_str.len().min(1000)]
            );
        }

        result.context("Failed to parse track from library response")
    }

    /// List available Spotify Connect devices via a temporary observer registration.
    pub async fn get_devices(&self) -> Result<Vec<SpotifyDevice>> {
        let cluster = self
            .observe_connect_cluster()
            .await
            .context("Failed to observe Spotify Connect cluster")?;
        let devices = Self::parse_connect_devices(&cluster);
        log::info!(
            "[spotify_webapi] Observed {} Spotify Connect devices",
            devices.len()
        );
        Ok(devices)
    }

    /// Play a specific track by URI via Spotify Connect API.
    /// This sends the command over the network so the desktop app window is NOT activated.
    pub async fn play_track(&self, uri: &str) -> Result<()> {
        let body = serde_json::json!({"uris": [uri]});

        self.send_with_retry("play_track", |token| {
            self.client
                .put("https://api.spotify.com/v1/me/player/play")
                .bearer_auth(token)
                .json(&body)
        })
        .await
        .context("Spotify play_track failed")?;

        log::info!("[spotify_webapi] Playing track via Web API: {}", uri);
        Ok(())
    }

    /// Transfer playback using Spotify's internal connect-state API.
    pub async fn transfer_playback(&self, device_id: &str) -> Result<()> {
        let token = self.auth.get_access_token().await?;
        let cluster = self
            .observe_connect_cluster()
            .await
            .context("Failed to observe Spotify Connect cluster before transfer")?;

        let target_device_id = device_id
            .split("::a_")
            .next()
            .filter(|id| !id.is_empty())
            .unwrap_or(device_id);
        let source_device_id = Self::connect_active_device_id(&cluster).with_context(|| {
            format!(
                "No active Spotify Connect device found while transferring to {}",
                target_device_id
            )
        })?;

        let url = format!(
            "{}/v1/connect/transfer/from/{}/to/{}",
            CONNECT_STATE_API, source_device_id, target_device_id
        );
        let body = serde_json::json!({
            "transfer_options": {
                "restore_paused": "restore"
            }
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to request Spotify Connect transfer")?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "Spotify transfer_playback failed ({}): {}",
                status,
                Self::summarize_body(&text)
            );
        }

        log::info!(
            "[spotify_webapi] Transferred playback from {} to {}",
            source_device_id,
            target_device_id
        );
        Ok(())
    }

    async fn observe_connect_cluster(&self) -> Result<Value> {
        let token = self.auth.get_access_token().await?;
        let dealer_url = Url::parse_with_params(DEALER_API, &[("access_token", token.as_str())])?;
        let (mut dealer_socket, _) = connect_async(dealer_url.as_str())
            .await
            .context("Failed to connect to Spotify dealer websocket")?;

        let connection_id = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(message) = dealer_socket.next().await {
                let message = message.context("Dealer websocket returned an error")?;
                let Message::Text(text) = message else {
                    continue;
                };

                let payload: Value = serde_json::from_str(text.as_ref())
                    .context("Failed to parse dealer websocket payload")?;
                if let Some(id) = payload
                    .pointer("/headers/Spotify-Connection-Id")
                    .and_then(Value::as_str)
                {
                    return Ok(id.to_string());
                }
            }

            anyhow::bail!("Dealer websocket closed before sending Spotify-Connection-Id")
        })
        .await
        .context("Timed out waiting for Spotify dealer connection id")??;

        let observer_id = Self::random_observer_id();
        let register_url = format!("{}/v1/devices/{}", CONNECT_STATE_API, observer_id);
        let register_body = serde_json::json!({
            "member_type": "CONNECT_STATE",
            "device": {
                "device_info": {
                    "capabilities": {
                        "can_be_player": false,
                        "hidden": true,
                        "needs_full_player_state": true
                    }
                }
            }
        });

        let response = self
            .client
            .put(&register_url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Spotify-Connection-Id", &connection_id)
            .json(&register_body)
            .send()
            .await
            .context("Failed to register Spotify Connect observer")?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        let cluster_result = if !status.is_success() {
            Err(anyhow::anyhow!(
                "Spotify Connect observer registration failed ({}): {}",
                status,
                Self::summarize_body(&text)
            ))
        } else {
            serde_json::from_str(&text).context("Failed to parse Spotify Connect cluster response")
        };

        if let Err(err) = self.deregister_connect_observer(&token, &observer_id).await {
            log::debug!(
                "[spotify_webapi] Failed to deregister Spotify Connect observer {}: {}",
                observer_id,
                err
            );
        }

        cluster_result
    }

    async fn deregister_connect_observer(&self, token: &str, observer_id: &str) -> Result<()> {
        let url = format!("{}/v1/devices/{}", CONNECT_STATE_API, observer_id);
        let response = self
            .client
            .delete(&url)
            .bearer_auth(token)
            .send()
            .await
            .context("Failed to deregister Spotify Connect observer")?;

        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }

        let text = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "Spotify Connect observer deregistration failed ({}): {}",
            status,
            Self::summarize_body(&text)
        )
    }

    fn random_observer_id() -> String {
        let bytes: [u8; 16] = rand::random();
        let hex = bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        format!("hobs_{}", hex)
    }

    fn parse_connect_devices(cluster: &Value) -> Vec<SpotifyDevice> {
        let active_device_id = Self::connect_active_device_id(cluster);
        let Some(devices) = cluster.get("devices").and_then(Value::as_object) else {
            return Vec::new();
        };

        let mut out = Vec::with_capacity(devices.len());
        for (fallback_id, raw_device) in devices {
            let hidden = raw_device
                .pointer("/capabilities/hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if hidden {
                continue;
            }

            let id = raw_device
                .get("device_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .unwrap_or(fallback_id)
                .to_string();
            let name = Self::connect_device_name(raw_device);
            let device_type = raw_device
                .get("device_type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_lowercase();
            let is_active = active_device_id
                .as_deref()
                .map(|active_id| active_id == id)
                .unwrap_or(false);
            let volume_percent = if raw_device
                .pointer("/capabilities/disable_volume")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                None
            } else {
                raw_device.get("volume").and_then(Value::as_u64).map(|raw| {
                    let scaled = raw.min(65_535);
                    (((scaled * 100) + 32_767) / 65_535) as u32
                })
            };

            out.push(SpotifyDevice {
                id,
                name,
                device_type,
                is_active,
                volume_percent,
            });
        }

        out.sort_by(|a, b| {
            b.is_active
                .cmp(&a.is_active)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    fn connect_active_device_id(cluster: &Value) -> Option<String> {
        cluster
            .get("active_device_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                cluster
                    .pointer("/player_state/play_origin/device_identifier")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
            })
            .or_else(|| {
                let devices = cluster.get("devices").and_then(Value::as_object)?;
                if devices.len() == 1 {
                    devices.keys().next().cloned()
                } else {
                    None
                }
            })
    }

    fn connect_device_name(device: &Value) -> String {
        let alias_key = device.get("selected_alias_id").and_then(|value| {
            value
                .as_u64()
                .map(|n| n.to_string())
                .or_else(|| value.as_str().map(ToString::to_string))
        });
        if let Some(alias_key) = alias_key {
            let alias_pointer = format!("/device_aliases/{}/display_name", alias_key);
            if let Some(name) = device.pointer(&alias_pointer).and_then(Value::as_str) {
                return name.to_string();
            }
        }

        device
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("Unknown device")
            .to_string()
    }

    // ── Response parsers ────────────────────────────────────────────────

    /// Parse a track from a GraphQL search response item.
    /// Expected structure: { "item": { "data": { "uri", "name", "artists", ... } } }
    fn graphql_search_track_to_result(item: &Value) -> Option<SearchResult> {
        let data = item.pointer("/item/data")?;
        Self::graphql_track_data_to_result(data)
    }

    /// Parse a track from a GraphQL library response item.
    /// Handles multiple response structures:
    /// - { "track": { "uri", "name", ... } }
    /// - { "track": { "data": { "uri", "name", ... } } }
    /// - { "itemV2": { "data": { "uri", "name", ... } } }
    /// - { "uri", "name", ... } (direct)
    fn graphql_library_track_to_result(item: &Value) -> Option<SearchResult> {
        // Try /track directly, then /track/data
        if let Some(track) = item.get("track") {
            if track.get("uri").is_some() {
                return Self::graphql_track_data_to_result(track);
            }
            if let Some(data) = track.get("data") {
                if let Some(result) = Self::graphql_track_data_to_result(data) {
                    return Some(result);
                }
                // Handle Spotify _uri pattern: URI at track._uri, metadata in track.data
                if let Some(uri) = track.get("_uri").and_then(|v| v.as_str()) {
                    let mut merged = data.clone();
                    if let Some(obj) = merged.as_object_mut() {
                        obj.insert("uri".to_string(), Value::String(uri.to_string()));
                    }
                    return Self::graphql_track_data_to_result(&merged);
                }
            }
        }
        // Try /itemV2/data pattern
        if let Some(item_v2) = item.get("itemV2") {
            if let Some(data) = item_v2.get("data") {
                return Self::graphql_track_data_to_result(data);
            }
            if item_v2.get("uri").is_some() {
                return Self::graphql_track_data_to_result(item_v2);
            }
        }
        // Try /item/data pattern (same as search)
        if let Some(inner) = item.get("item") {
            if let Some(data) = inner.get("data") {
                return Self::graphql_track_data_to_result(data);
            }
        }
        // Try direct (item itself has uri/name)
        if item.get("uri").is_some() {
            return Self::graphql_track_data_to_result(item);
        }
        None
    }

    /// Parse a track from spclient collection paging response item.
    /// Handles structures like:
    /// - { "uri": "spotify:track:xxx", "name": "...", ... }
    /// - { "trackUri": "spotify:track:xxx", "trackMetadata": { ... } }
    fn spclient_collection_item_to_result(item: &Value) -> Option<SearchResult> {
        // Try direct uri/name
        if let Some(uri) = item.get("uri").and_then(|v| v.as_str()) {
            if uri.starts_with("spotify:track:") {
                let id = uri.strip_prefix("spotify:track:").unwrap_or("");
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let artist = item
                    .get("artist")
                    .or_else(|| item.get("artistName"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                return Some(SearchResult {
                    id: id.to_string(),
                    name: name.to_string(),
                    artist: artist.to_string(),
                    album: item
                        .get("album")
                        .or_else(|| item.get("albumName"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    album_art_url: None,
                    duration_ms: 0,
                    uri: uri.to_string(),
                });
            }
        }
        // Try trackUri pattern
        if let Some(uri) = item.get("trackUri").and_then(|v| v.as_str()) {
            if uri.starts_with("spotify:track:") {
                let id = uri.strip_prefix("spotify:track:").unwrap_or("");
                let metadata = item.get("trackMetadata");
                let name = metadata
                    .and_then(|m| m.get("trackName").or_else(|| m.get("name")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let artist = metadata
                    .and_then(|m| m.get("artistName").or_else(|| m.get("artist")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                return Some(SearchResult {
                    id: id.to_string(),
                    name: name.to_string(),
                    artist: artist.to_string(),
                    album: metadata
                        .and_then(|m| m.get("albumName").or_else(|| m.get("album")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    album_art_url: None,
                    duration_ms: 0,
                    uri: uri.to_string(),
                });
            }
        }
        // Try nested item patterns
        if let Some(track) = item.get("track").or_else(|| item.get("itemV2")) {
            return Self::spclient_collection_item_to_result(track);
        }
        if let Some(data) = item.get("data") {
            return Self::graphql_track_data_to_result(data);
        }
        None
    }

    /// Convert a GraphQL track data object into SearchResult.
    fn graphql_track_data_to_result(data: &Value) -> Option<SearchResult> {
        let uri = data.get("uri")?.as_str()?;
        let id = uri
            .strip_prefix("spotify:track:")
            .unwrap_or(data.get("id").and_then(|v| v.as_str()).unwrap_or(""));
        let name = data.get("name")?.as_str()?;

        let artists: Vec<&str> = data
            .pointer("/artists/items")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        a.pointer("/profile/name")
                            .or_else(|| a.get("name"))
                            .and_then(|n| n.as_str())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let album_name = data
            .pointer("/albumOfTrack/name")
            .or_else(|| data.pointer("/album/name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let album_art_url = data
            .pointer("/albumOfTrack/coverArt/sources")
            .or_else(|| data.pointer("/album/images"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|src| src.get("url").and_then(|v| v.as_str()))
            .map(String::from);

        let duration_ms = data
            .pointer("/duration/totalMilliseconds")
            .or_else(|| data.get("duration_ms"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Some(SearchResult {
            id: id.to_string(),
            name: name.to_string(),
            artist: artists.join(", "),
            album: album_name.to_string(),
            album_art_url,
            duration_ms,
            uri: uri.to_string(),
        })
    }

    // ── Retry logic ─────────────────────────────────────────────────────

    async fn send_with_retry<F>(&self, operation: &str, mut build: F) -> Result<Response>
    where
        F: FnMut(&str) -> RequestBuilder,
    {
        let mut token = self.auth.get_access_token().await?;

        for attempt in 0..MAX_RETRY_ATTEMPTS {
            let attempt_no = attempt + 1;
            let response = match build(&token).send().await {
                Ok(resp) => resp,
                Err(err) => {
                    if attempt_no < MAX_RETRY_ATTEMPTS {
                        let delay = Self::fallback_retry_delay(attempt);
                        log::warn!(
                            "[spotify_webapi] {} network error on attempt {}/{}: {}. Retrying in {}ms",
                            operation,
                            attempt_no,
                            MAX_RETRY_ATTEMPTS,
                            err,
                            delay.as_millis()
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(err)
                        .with_context(|| format!("Spotify {} request failed", operation));
                }
            };

            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }

            let should_retry_status =
                status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            let retry_delay = if should_retry_status {
                Some(Self::retry_delay_from_headers(response.headers(), attempt))
            } else {
                None
            };
            let body = response.text().await.unwrap_or_default();
            let body_summary = Self::summarize_body(&body);

            if status == StatusCode::UNAUTHORIZED && attempt_no < MAX_RETRY_ATTEMPTS {
                log::warn!(
                    "[spotify_webapi] {} got 401 on attempt {}/{}. Refreshing token. body={}",
                    operation,
                    attempt_no,
                    MAX_RETRY_ATTEMPTS,
                    body_summary
                );
                self.auth.invalidate_token().await;
                token = self.auth.get_access_token().await?;
                continue;
            }

            if let Some(delay) = retry_delay {
                if attempt_no < MAX_RETRY_ATTEMPTS {
                    log::warn!(
                        "[spotify_webapi] {} got {} on attempt {}/{}. Retrying in {}ms. body={}",
                        operation,
                        status,
                        attempt_no,
                        MAX_RETRY_ATTEMPTS,
                        delay.as_millis(),
                        body_summary
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }

            return Err(anyhow::anyhow!(
                "Spotify {} failed ({}): {}",
                operation,
                status,
                body_summary
            ));
        }

        Err(anyhow::anyhow!(
            "Spotify {} failed after {} attempts",
            operation,
            MAX_RETRY_ATTEMPTS
        ))
    }

    fn retry_delay_from_headers(headers: &HeaderMap, attempt: usize) -> Duration {
        if let Some(ms) = headers
            .get("retry-after-ms")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            return Duration::from_millis(ms.clamp(250, 30_000));
        }

        if let Some(seconds) = headers
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            return Duration::from_secs(seconds.clamp(1, 30));
        }

        Self::fallback_retry_delay(attempt)
    }

    fn fallback_retry_delay(attempt: usize) -> Duration {
        const DELAYS_MS: [u64; MAX_RETRY_ATTEMPTS] = [600, 1500, 3000];
        Duration::from_millis(DELAYS_MS[attempt.min(DELAYS_MS.len() - 1)])
    }

    fn summarize_body(body: &str) -> String {
        let text = body.trim();
        if text.is_empty() {
            return "<empty body>".to_string();
        }

        let mut out = text.chars().take(240).collect::<String>();
        if text.chars().count() > 240 {
            out.push_str("...");
        }
        out
    }
}
