use serde::{Deserialize, Serialize};

const VERSION_URL: &str = "https://www.expotify.live/version.json";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Clone, Debug)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub download_url: String,
    pub release_url: String,
}

#[derive(Deserialize)]
struct VersionManifest {
    version: String,
    download_url: String,
    release_url: String,
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

pub async fn check_for_update() -> Result<UpdateInfo, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let manifest: VersionManifest = client
        .get(VERSION_URL)
        .header("User-Agent", format!("Expotify/{}", CURRENT_VERSION))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let has_update = is_newer(&manifest.version, CURRENT_VERSION);

    Ok(UpdateInfo {
        has_update,
        latest_version: manifest.version,
        download_url: manifest.download_url,
        release_url: manifest.release_url,
    })
}
