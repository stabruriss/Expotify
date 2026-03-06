use std::path::PathBuf;

/// Anthropic authentication via local API key.
/// Reads from `~/.claude/anthropic_key.sh` (echoed value) or `ANTHROPIC_API_KEY` env var.
pub struct AnthropicAuth {
    api_key: Option<String>,
}

impl AnthropicAuth {
    pub fn new() -> Self {
        let api_key = Self::read_key_from_script()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

        if api_key.is_some() {
            log::info!("[anthropic_auth] API key detected");
        } else {
            log::info!("[anthropic_auth] No API key found");
        }

        Self { api_key }
    }

    pub fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Parse `~/.claude/anthropic_key.sh` which contains `echo "sk-ant-..."`.
    fn read_key_from_script() -> Option<String> {
        let path = dirs::home_dir()?.join(".claude").join("anthropic_key.sh");
        Self::parse_key_file(&path)
    }

    fn parse_key_file(path: &PathBuf) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        // Find the echo'd string: `echo "..."` or `echo '...'`
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("echo ") {
                let key = rest
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                if key.starts_with("sk-ant-") {
                    return Some(key);
                }
            }
        }
        None
    }
}

impl Default for AnthropicAuth {
    fn default() -> Self {
        Self::new()
    }
}
