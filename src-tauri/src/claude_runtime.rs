use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ClaudeAuthenticationError(pub String);

pub struct ClaudeRuntime {
    helper: PathBuf,
    config_dir: PathBuf,
    operation: Mutex<()>,
    login_gate: Mutex<()>,
    login_cancel: Mutex<Option<oneshot::Sender<()>>>,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    struct Fixture {
        dir: PathBuf,
        runtime: Arc<ClaudeRuntime>,
    }
    impl Fixture {
        fn new(script: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("expotify-runtime-test-{}", rand::random::<u64>()));
            std::fs::create_dir(&dir).unwrap();
            let helper = dir.join("helper");
            std::fs::write(
                &helper,
                format!("#!/bin/sh\n/bin/cat >/dev/null\n{script}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let runtime = Arc::new(ClaudeRuntime::new(helper, dir.join("config")).unwrap());
            Self { dir, runtime }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn isolated_runtime_uses_system_path_and_private_config() {
        let fixture = Fixture::new("printf '{\"ok\":true,\"data\":{\"path\":\"%s\"}}' \"$PATH\"");
        let result = fixture
            .runtime
            .call(json!({"action":"status"}), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(result["path"], "/usr/bin:/bin:/usr/sbin:/sbin");
        assert_eq!(
            std::fs::metadata(fixture.dir.join("config"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn malformed_helper_response_is_not_success() {
        let fixture = Fixture::new("printf 'not json'");
        let error = fixture
            .runtime
            .call(json!({"action":"status"}), Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid response"), "{error}");
    }

    #[tokio::test]
    async fn timeout_kills_native_descendants() {
        let fixture = Fixture::new("/bin/sleep 60 &\nprintf '%s' $! > descendant\nwait");
        let error = fixture
            .runtime
            .call(json!({"action":"status"}), Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        let pid: i32 = std::fs::read_to_string(fixture.dir.join("config/descendant"))
            .unwrap()
            .parse()
            .unwrap();
        let mut exited = false;
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } == -1 {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(exited, "the helper's native child must be terminated");
    }

    #[tokio::test]
    async fn cancelling_login_allows_a_new_attempt() {
        let fixture = Fixture::new("/bin/sleep 60");
        for _ in 0..2 {
            let runtime = Arc::clone(&fixture.runtime);
            let login = tokio::spawn(async move { runtime.login().await });
            for _ in 0..100 {
                if fixture.runtime.login_cancel.lock().await.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(fixture
                .runtime
                .login()
                .await
                .unwrap_err()
                .to_string()
                .contains("already in progress"));
            fixture.runtime.cancel_login().await;
            let result = tokio::time::timeout(Duration::from_secs(2), login)
                .await
                .unwrap()
                .unwrap();
            assert!(result.unwrap_err().to_string().contains("cancelled"));
        }
    }
}

// The SDK starts a native child. Terminate the whole group on timeout/cancel,
// including when the awaiting Tauri command is dropped.
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

impl ClaudeRuntime {
    pub fn new(helper: PathBuf, config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            helper,
            config_dir,
            operation: Mutex::new(()),
            login_gate: Mutex::new(()),
            login_cancel: Mutex::new(None),
        })
    }

    pub async fn call(&self, mut request: Value, timeout: Duration) -> Result<Value> {
        let login = request["action"] == "login";
        let _operation = tokio::time::timeout(timeout, self.operation.lock())
            .await
            .context("Claude runtime is busy. Please try again.")?;
        request["configDir"] = json!(self.config_dir);
        let payload = serde_json::to_vec(&request)?;
        if payload.len() > 4 * 1024 * 1024 {
            bail!("Claude request is too large");
        }
        let mut command = Command::new(&self.helper);
        command.env_clear();
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .current_dir(&self.config_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .context("Could not start the bundled Claude runtime. Reinstall Expotify.")?;
        let _group = ProcessGroup(child.id().context("Claude runtime did not start")?);
        let operation = async move {
            let mut stdin = child
                .stdin
                .take()
                .context("Claude runtime input unavailable")?;
            stdin.write_all(&payload).await?;
            drop(stdin);
            let mut stdout = child
                .stdout
                .take()
                .context("Claude runtime output unavailable")?
                .take(8 * 1024 * 1024 + 1);
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).await?;
            if output.len() > 8 * 1024 * 1024 {
                bail!("Claude runtime response is too large");
            }
            let status = child.wait().await?;
            let response: Value = serde_json::from_slice(&output)
                .context("Claude runtime returned an invalid response")?;
            if !status.success() || response["ok"] != true {
                let message = response["error"]
                    .as_str()
                    .unwrap_or("Claude runtime failed");
                if response["code"] == "authentication_required" {
                    return Err(ClaudeAuthenticationError(message.to_owned()).into());
                }
                bail!("{message}");
            }
            Ok(response["data"].clone())
        };
        tokio::time::timeout(timeout, operation)
            .await
            .context(if login {
                "Claude sign-in timed out waiting for the browser callback. Retry with your default browser and allow the localhost callback."
            } else { "Claude request timed out. Please try again." })?
    }

    pub async fn login(&self) -> Result<()> {
        let _login = self
            .login_gate
            .try_lock()
            .context("Claude sign-in is already in progress")?;
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.login_cancel.lock().await;
            *pending = Some(sender);
        }
        let result = tokio::select! {
            response = self.call(json!({"action":"login"}), Duration::from_secs(300)) => response.map(|_| ()),
            _ = receiver => Err(anyhow::anyhow!("Claude sign-in cancelled")),
        };
        *self.login_cancel.lock().await = None;
        result
    }

    pub async fn cancel_login(&self) {
        if let Some(sender) = self.login_cancel.lock().await.take() {
            let _ = sender.send(());
        }
    }

    pub async fn is_authenticated(&self) -> Result<bool> {
        Ok(self
            .call(json!({"action":"status"}), Duration::from_secs(20))
            .await?["loggedIn"]
            == true)
    }

    pub async fn logout(&self) -> Result<()> {
        self.cancel_login().await;
        self.call(json!({"action":"logout"}), Duration::from_secs(30))
            .await?;
        Ok(())
    }
}
