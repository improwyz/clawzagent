//! Optional OpenAI Whisper STT + TTS for `/ws/voice`.
//!
//! Enable with `CLAWZ_VOICE_PROVIDER=openai` and `OPENAI_API_KEY`.

use clawz_core::error::{ClawzError, Result};

/// Voice pipeline backend selected via `CLAWZ_VOICE_PROVIDER`.
pub struct VoicePipeline;

impl VoicePipeline {
    pub fn enabled() -> bool {
        std::env::var("CLAWZ_VOICE_PROVIDER")
            .map(|v| v.eq_ignore_ascii_case("openai"))
            .unwrap_or(false)
    }

    fn api_key() -> Result<String> {
        std::env::var("OPENAI_API_KEY").map_err(|_| {
            ClawzError::Auth("OPENAI_API_KEY required when CLAWZ_VOICE_PROVIDER=openai".into())
        })
    }

    /// Transcribe audio bytes (wav, webm, mp3, …) via OpenAI Whisper.
    pub async fn transcribe(audio: &[u8], filename: &str) -> Result<String> {
        let key = Self::api_key()?;
        let part = reqwest::multipart::Part::bytes(audio.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| ClawzError::Provider(format!("voice multipart error: {e}")))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "whisper-1");

        let client = reqwest::Client::new();
        let resp = client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("whisper request failed: {e}")))?;

        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Provider(format!("whisper parse error: {e}")))?;

        if !status.is_success() {
            return Err(ClawzError::Provider(format!(
                "whisper failed ({status}): {body}"
            )));
        }

        body["text"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ClawzError::Provider("whisper response missing text".into()))
    }

    /// Synthesize speech (mp3) via OpenAI TTS.
    pub async fn synthesize(text: &str) -> Result<Vec<u8>> {
        let key = Self::api_key()?;
        let voice = std::env::var("CLAWZ_VOICE_TTS_VOICE").unwrap_or_else(|_| "alloy".into());
        let model = std::env::var("CLAWZ_VOICE_TTS_MODEL").unwrap_or_else(|_| "tts-1".into());

        let client = reqwest::Client::new();
        let resp = client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(key)
            .json(&serde_json::json!({
                "model": model,
                "voice": voice,
                "input": text,
                "response_format": "mp3",
            }))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("tts request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "tts failed ({status}): {body}"
            )));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ClawzError::Provider(format!("tts read body: {e}")))
    }
}
