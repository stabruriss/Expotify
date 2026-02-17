/// Keyword-based heuristic detection for instrumental tracks.
pub fn is_likely_instrumental(track_name: &str) -> bool {
    let name_lower = track_name.to_lowercase();

    let explicit_keywords = [
        "instrumental",
        "(inst)",
        "(inst.)",
        "- inst",
        "伴奏",
        "纯音乐",
        "off vocal",
        "カラオケ",
        "インスト",
        "(bgm)",
    ];

    for keyword in &explicit_keywords {
        if name_lower.contains(keyword) {
            return true;
        }
    }

    // Classical music title patterns — only flag as instrumental
    // when there's no vocal hint
    let classical_patterns = [
        "symphony no.",
        "concerto no.",
        "sonata no.",
        "etude no.",
        "prelude no.",
        "fugue no.",
        "nocturne no.",
        "waltz no.",
        "overture",
        "serenade no.",
        "交响曲",
        "协奏曲",
        "奏鸣曲",
        "练习曲",
        "前奏曲",
    ];

    let has_classical = classical_patterns
        .iter()
        .any(|p| name_lower.contains(p));

    if has_classical {
        let vocal_hints = ["vocal", "feat.", "feat ", "歌", "唱", "词", "aria", "咏叹调"];
        let has_vocal = vocal_hints.iter().any(|h| name_lower.contains(h));
        return !has_vocal;
    }

    false
}
