use crate::ai::tools::{ToolCall, ToolOutcome};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex, MutexGuard};

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

/// A shell script standing in for the bundled Claude helper, shared by tests across modules.
#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use super::ClaudeRuntime;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub(crate) struct FakeHelper {
        pub dir: PathBuf,
        pub runtime: Arc<ClaudeRuntime>,
    }
    impl FakeHelper {
        pub(crate) fn new(script: &str) -> Self {
            // Scripts that start by reading the request line manage stdin themselves;
            // the others drain it like the real helper does for single-response actions.
            let prelude = if script.starts_with("IFS= read") { "" } else { "/bin/cat >/dev/null\n" };
            let dir = std::env::temp_dir()
                .join(format!("expotify-runtime-test-{}", rand::random::<u64>()));
            std::fs::create_dir(&dir).unwrap();
            let helper = dir.join("helper");
            std::fs::write(
                &helper,
                format!("#!/bin/sh\n{prelude}{script}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let runtime = Arc::new(ClaudeRuntime::new(helper, dir.join("config")).unwrap());
            Self { dir, runtime }
        }
    }
    impl Drop for FakeHelper {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::test_support::FakeHelper as Fixture;
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

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
        // A freshly written script can take a moment to start under load; the pid file is
        // written as soon as it runs.
        let descendant = fixture.dir.join("config/descendant");
        let mut recorded = String::new();
        for _ in 0..500 {
            recorded = std::fs::read_to_string(&descendant).unwrap_or_default();
            if !recorded.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid: i32 = recorded.trim().parse().expect("the helper must record its child's pid");
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

    #[tokio::test]
    async fn native_session_relays_tool_calls_and_results() {
        // Fake helper: read the request line, ask for one tool call, echo the result back.
        let fixture = Fixture::new(
            "IFS= read -r request\nprintf '%s\\n' '{\"type\":\"tool_call\",\"id\":\"call-1\",\"name\":\"set_volume\",\"args\":{\"level\":30}}'\nIFS= read -r result\nprintf '{\"ok\":true,\"data\":{\"text\":\"done\",\"echo\":%s}}' \"$result\"",
        );
        let mut session = fixture
            .runtime
            .start(json!({"action":"prompt","protocol":"native"}), Duration::from_secs(5))
            .await
            .unwrap();
        let call = match session.next().await.unwrap() {
            HelperEvent::ToolCall(call) => call,
            HelperEvent::Final(_) => panic!("expected a tool call first"),
        };
        assert_eq!(call.name, "set_volume");
        assert_eq!(call.args["level"], 30);
        let outcome = ToolOutcome::failed(&call, "test", "Volume set to 30.");
        session.reply(&outcome).await.unwrap();
        match session.next().await.unwrap() {
            HelperEvent::Final(data) => {
                assert_eq!(data["text"], "done");
                assert_eq!(data["echo"]["id"], "call-1");
                assert_eq!(data["echo"]["ok"], false);
                assert_eq!(data["echo"]["output"], "Volume set to 30.");
            }
            HelperEvent::ToolCall(_) => panic!("expected the final response"),
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

fn timeout_message(login: bool) -> &'static str {
    if login {
        "Claude sign-in timed out waiting for the browser callback. Retry with your default browser and allow the localhost callback."
    } else {
        "Claude request timed out. Please try again."
    }
}

/// One line from the helper: either a tool call to execute in Rust, or the final response.
pub enum HelperEvent {
    ToolCall(ToolCall),
    Final(Value),
}

/// A running helper process. Holds the runtime's operation lock for its lifetime so one
/// credential-refresh owner exists at a time; dropping it kills the process group.
pub struct ClaudeSession<'a> {
    _operation: MutexGuard<'a, ()>,
    _group: ProcessGroup,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    bytes_read: usize,
    deadline: Instant,
    login: bool,
}

impl ClaudeSession<'_> {
    fn remaining(&self) -> Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("{}", timeout_message(self.login));
        }
        Ok(remaining)
    }

    /// Wait for the next event. Blank lines are skipped; the final line ends the process.
    pub async fn next(&mut self) -> Result<HelperEvent> {
        loop {
            let remaining = self.remaining()?;
            let line = tokio::time::timeout(remaining, self.lines.next_line())
                .await
                .map_err(|_| anyhow!(timeout_message(self.login)))??;
            let Some(line) = line else {
                let status = tokio::time::timeout(self.remaining()?, self.child.wait())
                    .await
                    .map_err(|_| anyhow!(timeout_message(self.login)))??;
                bail!("Claude runtime exited without a response ({status})");
            };
            self.bytes_read += line.len() + 1;
            if self.bytes_read > 8 * 1024 * 1024 {
                bail!("Claude runtime response is too large");
            }
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line)
                .context("Claude runtime returned an invalid response")?;
            if value["type"] == "tool_call" {
                let id = value["id"].as_str().unwrap_or_default();
                let name = value["name"].as_str().unwrap_or_default();
                if id.is_empty() || name.is_empty() {
                    bail!("Claude runtime sent an invalid tool call");
                }
                return Ok(HelperEvent::ToolCall(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    args: value["args"].clone(),
                }));
            }
            // Final response: close our side and reap the process.
            self.stdin.take();
            let status = tokio::time::timeout(self.remaining()?, self.child.wait())
                .await
                .map_err(|_| anyhow!(timeout_message(self.login)))??;
            if !status.success() || value["ok"] != true {
                let message = value["error"]
                    .as_str()
                    .unwrap_or("Claude runtime failed");
                if value["code"] == "authentication_required" {
                    return Err(ClaudeAuthenticationError(message.to_owned()).into());
                }
                bail!("{message}");
            }
            return Ok(HelperEvent::Final(value["data"].clone()));
        }
    }

    /// Send a tool outcome back to the helper (native tool-calling only).
    pub async fn reply(&mut self, outcome: &ToolOutcome) -> Result<()> {
        let remaining = self.remaining()?;
        let stdin = self
            .stdin
            .as_mut()
            .context("Claude runtime tool channel is closed")?;
        let line = serde_json::to_string(&json!({
            "type": "tool_result",
            "id": outcome.call_id,
            "ok": outcome.ok,
            "output": outcome.output,
        }))?;
        tokio::time::timeout(remaining, async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        })
        .await
        .map_err(|_| anyhow!(timeout_message(self.login)))??;
        Ok(())
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

    /// Spawn the helper for one request. Non-native requests get their stdin closed right
    /// away (single response); native tool-calling requests keep it open so tool results
    /// can be written back. The returned session owns the process: dropping it kills the
    /// whole process group.
    pub async fn start(&self, mut request: Value, timeout: Duration) -> Result<ClaudeSession<'_>> {
        let login = request["action"] == "login";
        let keep_stdin = request["protocol"] == "native";
        let operation = tokio::time::timeout(timeout, self.operation.lock())
            .await
            .context("Claude runtime is busy. Please try again.")?;
        request["configDir"] = json!(self.config_dir);
        let mut payload = serde_json::to_vec(&request)?;
        if payload.len() > 4 * 1024 * 1024 {
            bail!("Claude request is too large");
        }
        payload.push(b'\n');
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
        let group = ProcessGroup(child.id().context("Claude runtime did not start")?);
        let deadline = Instant::now() + timeout;
        let mut stdin = child
            .stdin
            .take()
            .context("Claude runtime input unavailable")?;
        tokio::time::timeout(timeout, async {
            stdin.write_all(&payload).await?;
            stdin.flush().await
        })
        .await
        .map_err(|_| anyhow!(timeout_message(login)))??;
        let stdin = if keep_stdin {
            Some(stdin)
        } else {
            drop(stdin);
            None
        };
        let stdout = child
            .stdout
            .take()
            .context("Claude runtime output unavailable")?;
        Ok(ClaudeSession {
            _operation: operation,
            _group: group,
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            bytes_read: 0,
            deadline,
            login,
        })
    }

    /// Single request / single response. Tool calls are not accepted on this path.
    pub async fn call(&self, request: Value, timeout: Duration) -> Result<Value> {
        let mut session = self.start(request, timeout).await?;
        loop {
            match session.next().await? {
                HelperEvent::Final(data) => return Ok(data),
                HelperEvent::ToolCall(call) => {
                    let refused = ToolOutcome::failed(
                        &call,
                        "tools_unavailable",
                        "Tool calls are not accepted on this channel.",
                    );
                    session.reply(&refused).await?;
                }
            }
        }
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
