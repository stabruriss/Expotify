use anyhow::{Context, Result};
use std::process::Command;

use super::types::TrackInfo;

/// Check if Spotify desktop app is currently running
pub fn is_spotify_running() -> bool {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to (name of processes) contains "Spotify""#)
        .output();

    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

/// Get the current playing track from Spotify via AppleScript
pub fn get_current_track() -> Result<Option<TrackInfo>> {
    if !is_spotify_running() {
        return Ok(None);
    }

    let script = r#"
tell application "Spotify"
    if player state is stopped then
        return "STOPPED"
    end if
    set tid to id of current track
    set tname to name of current track
    set tartist to artist of current track
    set talbum to album of current track
    set tart to artwork url of current track
    set tdur to duration of current track
    set turl to spotify url of current track
    set ppos to player position
    set pstate to player state
    return tid & tab & tname & tab & tartist & tab & talbum & tab & tart & tab & tdur & tab & turl & tab & ppos & tab & pstate
end tell
"#;

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("Failed to execute osascript")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not running") || stderr.contains("Connection is invalid") {
            return Ok(None);
        }
        anyhow::bail!("AppleScript error: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if stdout == "STOPPED" || stdout.is_empty() {
        return Ok(None);
    }

    let parts: Vec<&str> = stdout.split('\t').collect();
    if parts.len() < 9 {
        anyhow::bail!("Unexpected AppleScript output format: {}", stdout);
    }

    let duration_ms: u64 = parts[5].parse().unwrap_or(0);
    let player_position_secs: f64 = parts[7].parse().unwrap_or(0.0);
    let is_playing = parts[8] == "playing";

    let raw_id = parts[0];
    let id = raw_id
        .strip_prefix("spotify:track:")
        .unwrap_or(raw_id)
        .to_string();

    Ok(Some(TrackInfo {
        id,
        name: parts[1].to_string(),
        artist: parts[2].to_string(),
        album: parts[3].to_string(),
        album_art_url: if parts[4].is_empty() {
            None
        } else {
            Some(parts[4].to_string())
        },
        duration_ms,
        progress_ms: (player_position_secs * 1000.0) as u64,
        is_playing,
        spotify_url: if parts[6].is_empty() {
            None
        } else {
            Some(parts[6].to_string())
        },
        ai_description: None,
        ai_used_web_search: false,
    }))
}

/// Control Spotify playback via AppleScript
pub fn spotify_play_pause() -> Result<()> {
    run_spotify_command("playpause")
}

pub fn spotify_next_track() -> Result<()> {
    run_spotify_command("next track")
}

pub fn spotify_previous_track() -> Result<()> {
    run_spotify_command("previous track")
}

fn run_spotify_command(cmd: &str) -> Result<()> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }
    let script = format!(r#"tell application "Spotify" to {}"#, cmd);
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("Failed to execute osascript")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("AppleScript error: {}", stderr);
    }
    Ok(())
}
