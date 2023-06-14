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
pub(crate) struct ConversionRequest<'a> {
    pub audio_data: &'a [types::WhisperAudioSample],
    pub previous_tokens: Option<&'a [types::WhisperToken]>,
    pub response_queue: tokio::sync::oneshot::Sender<ConversionResponse>,
    pub timestamp_start: DiscordRtcTimestamp,
    pub user_id: UserId,
}

#[derive(Debug)]
pub(crate) struct ConversionResponse {
    pub finalized_segment_count: usize,
    pub message: crate::api::api_types::TranscribedMessage,
    pub tokens: Vec<types::WhisperToken>,
}
