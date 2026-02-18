use serde::{Deserialize, Serialize};

const GITHUB_REPO: &str = "stabruriss/Expotify";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Clone, Debug)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub download_url: String,
    pub release_url: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
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
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::new();
    let release: GitHubRelease = client
        .get(&url)
        .header("User-Agent", format!("Expotify/{}", CURRENT_VERSION))
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let dmg_url = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".dmg"))
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_default();

    let has_update = is_newer(&release.tag_name, CURRENT_VERSION);

    Ok(UpdateInfo {
        has_update,
        latest_version: release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name).to_string(),
        download_url: dmg_url,
        release_url: release.html_url,
    })
}
