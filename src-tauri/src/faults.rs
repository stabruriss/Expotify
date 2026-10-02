//! Debug-only fault injection for local test builds.
//!
//! `EXPOTIFY_FAULTS=name1,name2=value` names the faults active for the lifetime of the
//! process; release builds never activate any. Nothing is persisted: relaunch without the
//! variable to recover. Known names:
//! - `spotify_not_connected`: the tool executor reports Spotify as not connected for every
//!   action that needs the Spotify Web API (credentials are untouched).
//! - `tool_exec_fail`: every validated tool call fails with `injected_failure` before running.
//! - `tool_delay[=seconds]`: the executor pauses after each executed action (default 5 s) so a
//!   tester can cancel or send a new request between the actions of one request.
//!
//! Other modules may define their own names (for example speech-synthesis faults) through
//! the same `active` / `value` checks.

pub fn active(name: &str) -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    active_in(std::env::var("EXPOTIFY_FAULTS").ok().as_deref(), name)
}

/// The `=value` part of an active fault, if it has one (`None` when inactive or bare).
pub fn value(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    value_in(std::env::var("EXPOTIFY_FAULTS").ok().as_deref(), name)
}

fn entries(list: Option<&str>) -> impl Iterator<Item = (&str, Option<&str>)> {
    list.unwrap_or("").split(',').filter_map(|entry| {
        let entry = entry.trim();
        if entry.is_empty() {
            return None;
        }
        Some(match entry.split_once('=') {
            Some((name, value)) => (name.trim(), Some(value.trim())),
            None => (entry, None),
        })
    })
}

fn active_in(list: Option<&str>, name: &str) -> bool {
    entries(list).any(|(fault, _)| fault == name)
}

fn value_in(list: Option<&str>, name: &str) -> Option<String> {
    entries(list)
        .find(|(fault, _)| *fault == name)
        .and_then(|(_, value)| value.map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::{active_in, value_in};

    #[test]
    fn names_are_matched_exactly_in_a_comma_list() {
        assert!(active_in(
            Some("tts_hang, spotify_not_connected"),
            "spotify_not_connected"
        ));
        assert!(active_in(
            Some("spotify_not_connected"),
            "spotify_not_connected"
        ));
        assert!(!active_in(
            Some("spotify_not_connected_x"),
            "spotify_not_connected"
        ));
        assert!(!active_in(Some(""), "spotify_not_connected"));
        assert!(!active_in(None, "spotify_not_connected"));
    }

    #[test]
    fn values_are_read_from_name_equals_value_entries() {
        assert!(active_in(Some("tool_delay=15"), "tool_delay"));
        assert_eq!(
            value_in(Some("tool_delay=15, tts_hang"), "tool_delay").as_deref(),
            Some("15")
        );
        assert_eq!(value_in(Some("tool_delay"), "tool_delay"), None);
        assert_eq!(value_in(Some("tts_hang"), "tool_delay"), None);
        assert!(!active_in(Some("tool_delay=15"), "tool"));
    }
}
