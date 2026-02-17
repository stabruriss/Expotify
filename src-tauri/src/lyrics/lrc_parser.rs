use super::types::LyricsLine;

/// Parse LRC format text into timestamped lyrics lines.
pub fn parse_lrc(lrc_text: &str) -> Vec<LyricsLine> {
    let mut lines = Vec::new();

    for raw_line in lrc_text.lines() {
        let raw_line = raw_line.trim();
        if raw_line.is_empty() {
            continue;
        }
        lines.extend(parse_lrc_line(raw_line));
    }

    lines.sort_by_key(|l| l.time_ms);
    lines
}

/// Parse a single LRC line, which may have multiple timestamps.
fn parse_lrc_line(line: &str) -> Vec<LyricsLine> {
    let mut results = Vec::new();
    let mut timestamps: Vec<u64> = Vec::new();
    let mut remaining = line;

    // Extract all [mm:ss.xx] tags
    while remaining.starts_with('[') {
        if let Some(close) = remaining.find(']') {
            let tag = &remaining[1..close];
            if let Some(ms) = parse_timestamp(tag) {
                timestamps.push(ms);
            } else {
                // Metadata tag like [ti:], [ar:], skip entire line
                return Vec::new();
            }
            remaining = &remaining[close + 1..];
        } else {
            break;
        }
    }

    let text = remaining.trim().to_string();

    if !timestamps.is_empty() {
        for ts in timestamps {
            results.push(LyricsLine {
                time_ms: ts,
                text: text.clone(),
            });
        }
    }

    results
}

/// Parse timestamp "mm:ss.xx" or "mm:ss.xxx" or "mm:ss" to milliseconds.
fn parse_timestamp(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    let minutes: u64 = parts[0].parse().ok()?;
    let sec_parts: Vec<&str> = parts[1].split('.').collect();
    let seconds: u64 = sec_parts[0].parse().ok()?;

    let milliseconds = if sec_parts.len() > 1 {
        let frac = sec_parts[1];
        match frac.len() {
            1 => frac.parse::<u64>().ok()? * 100,
            2 => frac.parse::<u64>().ok()? * 10,
            3 => frac.parse::<u64>().ok()?,
            _ => frac[..3].parse::<u64>().ok()?,
        }
    } else {
        0
    };

    Some(minutes * 60_000 + seconds * 1_000 + milliseconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("00:00.00"), Some(0));
        assert_eq!(parse_timestamp("01:30.50"), Some(90_500));
        assert_eq!(parse_timestamp("01:30.500"), Some(90_500));
        assert_eq!(parse_timestamp("01:30"), Some(90_000));
        assert_eq!(parse_timestamp("03:45.12"), Some(225_120));
    }

    #[test]
    fn test_parse_lrc_line() {
        let lines = parse_lrc_line("[00:12.00]Hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time_ms, 12_000);
        assert_eq!(lines[0].text, "Hello world");
    }

    #[test]
    fn test_parse_multiple_timestamps() {
        let lines = parse_lrc_line("[00:12.00][01:30.00]Repeated line");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, 12_000);
        assert_eq!(lines[1].time_ms, 90_000);
    }

    #[test]
    fn test_skip_metadata() {
        let lines = parse_lrc_line("[ti:Song Title]");
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_empty_text() {
        let lines = parse_lrc_line("[00:30.00]");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "");
    }

    #[test]
    fn test_parse_full_lrc() {
        let lrc = "[00:05.00]Line one\n[00:10.00]Line two\n[00:01.00]Earlier line";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].time_ms, 1_000);
        assert_eq!(lines[0].text, "Earlier line");
        assert_eq!(lines[1].time_ms, 5_000);
        assert_eq!(lines[2].time_ms, 10_000);
    }
}
