use anyhow::{Context, Result};
use std::{
    process::{Command, Output, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use super::types::TrackInfo;

const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(5);
const OSASCRIPT_POLL_INTERVAL: Duration = Duration::from_millis(25);

static OSASCRIPT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn osascript_lock() -> &'static Mutex<()> {
    OSASCRIPT_LOCK.get_or_init(|| Mutex::new(()))
}

fn run_command_with_timeout(mut command: Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn command")?;

    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .context("Failed to wait for command status")?
            .is_some()
        {
            return child
                .wait_with_output()
                .context("Failed to collect command output");
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait_with_output();
            anyhow::bail!("command timed out after {}ms", timeout.as_millis());
        }

        thread::sleep(OSASCRIPT_POLL_INTERVAL);
    }
}

fn run_osascript(script: &str) -> Result<Output> {
    let _guard = osascript_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("osascript lock poisoned"))?;
    let wrapped_script = format!(
        "with timeout of {} seconds\n{}\nend timeout",
        OSASCRIPT_TIMEOUT.as_secs(),
        script
    );
    let mut command = Command::new("osascript");
    command.arg("-e").arg(&wrapped_script);
    run_command_with_timeout(command, OSASCRIPT_TIMEOUT).context("Failed to execute osascript")
}

/// Check if Spotify desktop app is currently running
pub fn is_spotify_running() -> bool {
    // Avoid System Events in the hot polling path. Repeated AppleScript UI
    // automation can trigger expensive macOS TCC/signing checks.
    Command::new("pgrep")
        .arg("-x")
        .arg("Spotify")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
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

    let output = run_osascript(script)?;

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
        ai_error: None,
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

pub fn spotify_pause() -> Result<()> {
    run_spotify_command("pause")
}

pub fn spotify_play() -> Result<()> {
    run_spotify_command("play")
}

/// Play a specific track by Spotify URI (without raising the Spotify window)
pub fn spotify_play_track(uri: &str) -> Result<()> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }

    // Validate URI format to prevent AppleScript injection
    if !uri.starts_with("spotify:track:") && !uri.starts_with("spotify:episode:") {
        anyhow::bail!("Invalid Spotify URI: {}", uri);
    }

    // Play track via AppleScript. `tell application "Spotify"` activates the window,
    // so we save the frontmost app, play, hide Spotify, and restore focus.
    let script = format!(
        r#"
tell application "System Events"
    set frontApp to name of first application process whose frontmost is true
end tell
tell application "Spotify" to play track "{}"
tell application "System Events"
    set visible of process "Spotify" to false
end tell
tell application frontApp to activate
"#,
        uri
    );

    let output = run_osascript(&script)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to play track: {}", stderr);
    }

    Ok(())
}

/// Shuffle play the user's liked songs collection
pub fn spotify_shuffle_collection() -> Result<()> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }

    let script = r#"
tell application "System Events"
    set frontApp to name of first application process whose frontmost is true
end tell
tell application "Spotify"
    set shuffling to true
    play track "spotify:collection:tracks"
end tell
tell application "System Events"
    set visible of process "Spotify" to false
end tell
tell application frontApp to activate
"#;

    let output = run_osascript(script)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to shuffle liked songs: {}", stderr);
    }

    Ok(())
}

/// Get Spotify's current volume (0-100)
pub fn get_spotify_volume() -> Result<u32> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }
    let script = r#"tell application "Spotify" to sound volume"#;
    let output = run_osascript(script)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("AppleScript error: {}", stderr);
    }
    let vol: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .context("Failed to parse volume")?;
    Ok(vol)
}

/// Set Spotify's volume (0-100)
pub fn set_spotify_volume(volume: u32) -> Result<()> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }
    let vol = volume.min(100);
    let script = format!(
        r#"tell application "Spotify" to set sound volume to {}"#,
        vol
    );
    let output = run_osascript(&script)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("AppleScript error: {}", stderr);
    }
    Ok(())
}

fn run_spotify_command(cmd: &str) -> Result<()> {
    if !is_spotify_running() {
        anyhow::bail!("Spotify is not running");
    }
    let script = format!(r#"tell application "Spotify" to {}"#, cmd);
    let output = run_osascript(&script)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("AppleScript error: {}", stderr);
    }
    Ok(())
}
