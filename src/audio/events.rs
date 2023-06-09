// Discord should give us 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks.  But write this more generally just in case
// that changes.

use std::time::{Duration, SystemTime};

use bytes::Bytes;
use whisper_rs::WhisperToken;

use crate::model::types::{DiscordAudioSample, DiscordRtcTimestamp, Transcription, UserId};

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub(crate) enum UserAudioEventType {
    Speaking,
    Silent,
    Idle,
}

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub(crate) struct UserAudioEvent {
    pub user_id: UserId,
    pub event_type: UserAudioEventType,
}

#[derive(Debug)]
pub(crate) struct DiscordAudioData {
    pub user_id: UserId,
    pub discord_audio: Vec<DiscordAudioSample>,
    pub rtc_timestamp: DiscordRtcTimestamp,
}

#[derive(Debug)]
pub(crate) struct TranscriptionRequest {
    pub audio_bytes: Bytes,
    pub audio_duration: Duration,
    pub previous_tokens: Vec<WhisperToken>,
    pub start_timestamp: SystemTime,
    pub user_id: UserId,
}

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord)]
pub(crate) struct TranscriptionResponse {
    pub transcript: Transcription,
}
