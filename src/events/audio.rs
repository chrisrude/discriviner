// Discord should give us 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks.  But write this more generally just in case
// that changes.

use crate::model::types::{self, DiscordAudioSample, DiscordRtcTimestamp, UserId};

#[derive(Debug)]
pub(crate) struct DiscordAudioData {
    pub audio: Vec<DiscordAudioSample>,
    pub timestamp: DiscordRtcTimestamp,
    pub user_id: UserId,
}

#[derive(Debug)]
pub struct VoiceActivityData {
    /// Sent when a user begins speaking or stops speaking.
    pub user_id: u64,
    pub speaking: bool,
}

#[derive(Debug)]
pub(crate) struct TranscriptionRequest {
    pub audio_data: bytes::Bytes,
    pub previous_tokens: Vec<types::WhisperToken>,
    pub response_queue: tokio::sync::oneshot::Sender<TranscriptionResponse>,
    pub start_timestamp: u64,
    pub user_id: UserId,
}

#[derive(Debug)]
pub(crate) struct TranscriptionResponse {
    pub message: crate::api::api_types::TranscribedMessage,
    pub tokens: Vec<types::WhisperToken>,
}
