use anyhow::{Context, Result};
use msedge_tts::tts::stream::{msedge_tts_split_async, SynthesizedResponse};
use msedge_tts::tts::SpeechConfig;
use msedge_tts::voice::Voice;
use std::future::Future;
use std::time::{Duration, Instant};

/// Default voice for TTS - Xiaoxiao handles both Chinese and English well
const DEFAULT_VOICE: &str = "zh-CN-XiaoxiaoNeural";
const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(30);
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

async fn with_timeout<T>(
    timeout: Duration,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, operation)
        .await
        .with_context(|| format!("Speech service did not respond within {} seconds. Check your connection and try again.", timeout.as_secs()))?
}

async fn request_audio(text: &str, timeout: Duration) -> Result<Vec<u8>> {
    if text.trim().is_empty() {
        anyhow::bail!("There is no text to read aloud");
    }
    let voice = Voice::from(DEFAULT_VOICE);
    let config = SpeechConfig::from(&voice);
    // The library inserts text directly into SSML.
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let started = Instant::now();
    with_timeout(timeout, async {
        if crate::faults::active("tts_unreachable") {
            anyhow::bail!("Could not connect to the speech service (local test fault)");
        }
        if crate::faults::active("tts_hang") {
            return std::future::pending::<Result<Vec<u8>>>().await;
        }
        let (mut sender, mut reader) = msedge_tts_split_async()
            .await
            .context("Could not connect to the speech service")?;
        let connected_ms = started.elapsed().as_millis();
        sender
            .send(&text, &config)
            .await
            .context("Could not request speech audio")?;
        let mut audio_bytes = Vec::new();
        let mut first_audio_ms = None;
        while reader.can_read().await {
            if let Some(SynthesizedResponse::AudioBytes(bytes)) = reader
                .read()
                .await
                .context("Could not receive speech audio")?
            {
                if !bytes.is_empty() && first_audio_ms.is_none() {
                    first_audio_ms = Some(started.elapsed().as_millis());
                }
                audio_bytes.extend(bytes);
            }
            // Also lets the deadline run if the remote end closes without turn.end.
            tokio::task::yield_now().await;
        }
        if audio_bytes.is_empty() {
            anyhow::bail!("Speech service returned no audio. Please try again.");
        }
        log::info!(
            "[TTS timing] connect_ms={connected_ms} first_audio_ms={} complete_ms={} bytes={}",
            first_audio_ms.unwrap_or_default(),
            started.elapsed().as_millis(),
            audio_bytes.len()
        );
        Ok(audio_bytes)
    })
    .await
}

/// Deadline cancellation drops the connection, including when the network stalls.
pub async fn synthesize(text: &str) -> Result<Vec<u8>> {
    // These names leave preflight healthy so Chat/Insight runtime failures can
    // also be exercised without changing the machine's network.
    if crate::faults::active("tts_synthesize_fail") {
        anyhow::bail!("Could not receive speech audio (local test fault)");
    }
    if crate::faults::active("tts_synthesize_hang") {
        return with_timeout(SYNTHESIS_TIMEOUT, std::future::pending()).await;
    }
    request_audio(text, SYNTHESIS_TIMEOUT).await
}

/// Test the actual synthesis path, rather than just a reachable host. No playback.
pub async fn check_available() -> Result<()> {
    request_audio("语音连接检查。", CHECK_TIMEOUT)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_request_fails_with_a_deadline() {
        let error = with_timeout::<()>(Duration::from_millis(10), std::future::pending())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Speech service did not respond"));
    }

    #[tokio::test]
    async fn request_error_is_preserved() {
        let error =
            with_timeout::<()>(CHECK_TIMEOUT, async { anyhow::bail!("connection refused") })
                .await
                .unwrap_err();
        assert_eq!(error.to_string(), "connection refused");
    }

    #[tokio::test]
    async fn empty_text_is_rejected_without_a_request() {
        assert!(synthesize(" \n ")
            .await
            .unwrap_err()
            .to_string()
            .contains("no text"));
    }
}
