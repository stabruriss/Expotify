use crate::claude_runtime::ClaudeRuntime;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

// Native Claude auth owns this isolated credential store and all token refreshes.
// The legacy App-owned refresh token is deliberately not imported or modified.
pub struct AnthropicAuth {
    pub runtime: Arc<ClaudeRuntime>,
    connected: Mutex<Option<bool>>,
}

impl AnthropicAuth {
    pub fn new(runtime: Arc<ClaudeRuntime>) -> Self {
        Self {
            runtime,
            connected: Mutex::new(None),
        }
    }
    pub async fn login(&self) -> Result<()> {
        self.runtime.login().await?;
        *self.connected.lock().await = Some(true);
        Ok(())
    }
    pub async fn clear_pending_oauth(&self) {
        self.runtime.cancel_login().await;
    }
    pub async fn invalidate(&self) {
        *self.connected.lock().await = None;
    }
    pub async fn is_authenticated(&self) -> bool {
        // The overlay polls this every ten seconds. As with OpenAI, this is a
        // local connection state, not a live entitlement or token-validity probe.
        let mut connected = self.connected.lock().await;
        if let Some(value) = *connected {
            return value;
        }
        match self.runtime.is_authenticated().await {
            Ok(value) => {
                *connected = Some(value);
                value
            }
            Err(error) => {
                log::warn!("Could not read Claude connection state: {error}");
                false
            }
        }
    }
    pub async fn logout(&self) -> Result<()> {
        self.runtime.logout().await?;
        *self.connected.lock().await = Some(false);
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn repeated_status_polling_initializes_native_auth_once() {
        let dir =
            std::env::temp_dir().join(format!("expotify-auth-test-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        std::fs::write(&helper, "#!/bin/sh\n/bin/cat >/dev/null\nprintf 'check\\n' >> checks\nprintf '{\"ok\":true,\"data\":{\"loggedIn\":true}}'\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Arc::new(ClaudeRuntime::new(helper, dir.join("config")).unwrap());
        let auth = AnthropicAuth::new(runtime);
        let (a, b) = tokio::join!(auth.is_authenticated(), auth.is_authenticated());
        assert!(a && b && auth.is_authenticated().await);
        assert_eq!(
            std::fs::read_to_string(dir.join("config/checks")).unwrap(),
            "check\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
