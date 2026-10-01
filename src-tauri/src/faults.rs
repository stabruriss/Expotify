//! Debug-only fault injection for local test builds.
//!
//! `EXPOTIFY_FAULTS=name1,name2` names the faults active for the lifetime of the process;
//! release builds never activate any. Nothing is persisted: relaunch without the variable to
//! recover. Known names:
//! - `spotify_not_connected`: the tool executor reports Spotify as not connected for every
//!   action that needs the Spotify Web API (credentials are untouched).
//! - `tool_exec_fail`: every validated tool call fails with `injected_failure` before running.
//!
//! Other modules may define their own names (for example speech-synthesis faults) through
//! the same `active` check.

pub fn active(name: &str) -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    active_in(std::env::var("EXPOTIFY_FAULTS").ok().as_deref(), name)
}

fn active_in(list: Option<&str>, name: &str) -> bool {
    list.map(|list| list.split(',').any(|fault| fault.trim() == name))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::active_in;

    #[test]
    fn names_are_matched_exactly_in_a_comma_list() {
        assert!(active_in(Some("tts_hang, spotify_not_connected"), "spotify_not_connected"));
        assert!(active_in(Some("spotify_not_connected"), "spotify_not_connected"));
        assert!(!active_in(Some("spotify_not_connected_x"), "spotify_not_connected"));
        assert!(!active_in(Some(""), "spotify_not_connected"));
        assert!(!active_in(None, "spotify_not_connected"));
    }
}
