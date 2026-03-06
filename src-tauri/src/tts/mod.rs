use anyhow::Result;
use msedge_tts::tts::client::{connect, SynthesizedAudio};
use msedge_tts::tts::SpeechConfig;
use msedge_tts::voice::Voice;

/// Default voice for TTS - Xiaoxiao handles both Chinese and English well
const DEFAULT_VOICE: &str = "zh-CN-XiaoxiaoNeural";

/// Synthesize text to MP3 audio bytes using Microsoft Edge TTS.
/// This is a blocking call - wrap in spawn_blocking from async context.
pub fn synthesize(text: &str) -> Result<Vec<u8>> {
    let voice = Voice::from(DEFAULT_VOICE);
    let config = SpeechConfig::from(&voice);
    let mut client = connect()?;
    let audio: SynthesizedAudio = client.synthesize(text, &config)?;
    Ok(audio.audio_bytes)
}
