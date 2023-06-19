// Discord should give us 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks.  But write this more generally just in case
// that changes.

use std::time::{Duration, SystemTime};

use whisper_rs::WhisperToken;

use crate::model::types::{
    DiscordAudioSample, DiscordRtcTimestamp, Transcription, UserId, WhisperAudioSample,
};

#[derive(Debug)]
pub(crate) struct DiscordAudioData {
    pub discord_audio: Vec<DiscordAudioSample>,
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
    pub audio_data: Vec<WhisperAudioSample>,
    pub audio_duration: Duration,
    pub previous_tokens: Vec<WhisperToken>,
    pub start_timestamp: SystemTime,
    pub user_id: UserId,
}

#[derive(Debug)]
pub(crate) struct TranscriptionResponse {
    pub transcript: Transcription,
}
