// Discord should give us 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks.  But write this more generally just in case
// that changes.

use std::sync::Mutex;

use tokio::{sync::oneshot, task::JoinHandle};

use crate::{
    api::api_types::TranscribedMessage,
    model::types::{
        DiscordAudioSample, DiscordRtcTimestamp, UserId, WhisperAudioSample, WhisperToken,
    },
};

pub(crate) struct DiscordVoiceData {
    pub audio: Vec<DiscordAudioSample>,
    pub timestamp: u32,
    pub ssrc: u32,
}

pub(crate) struct AudioBufferForUser {
    /// User ID of the user that this buffer is for.
    pub user_id: u64,

    /// The buffers themselves.  We use two buffers so that
    /// we can write without blocking the transcription worker.
    ///
    /// Only the current_write_buffer can be written to.
    /// The current_write_buffer mutex must be held when
    /// writing to the buffer or changing the active buffer.
    pub buffers: [AudioBuffer; 2],

    /// The index of the buffer we're currently writing to.
    /// (either 0 or 1).
    ///
    /// The other buffer is the one the transcription worker
    /// is reading from, if there is a transcription worker.
    ///
    /// The lock must be held when writing to the buffer or
    /// changing the active buffer.
    pub current_write_buffer: Mutex<usize>,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    pub last_tokens: std::collections::VecDeque<WhisperToken>, // TOKENS_TO_KEEP

    // worker used for transcription
    pub worker_task: Mutex<Option<JoinHandle<()>>>,
}

pub struct AudioBuffer {
    /// Audio data lives here.
    pub buffer: std::collections::VecDeque<WhisperAudioSample>,

    /// The timestamp of the first sample in the buffer.
    /// Taken from the RTC packet.
    pub head_timestamp: DiscordRtcTimestamp,

    /// The timestamp of the last sample in the buffer.
    pub last_write_timestamp: DiscordRtcTimestamp,

    /// The timestamp of the most recent sample we've sent
    /// to the transcription worker.
    pub last_transcription_timestamp: DiscordRtcTimestamp,
}

pub struct VoiceActivityData {
    /// Sent when a user begins speaking or stops speaking.
    pub user_id: u64,
    pub speaking: bool,
}

pub struct TranscriptionResponse {
    pub tokens: Vec<i32>,
    pub message: TranscribedMessage,
}

pub struct TranscriptionRequest<'a> {
    pub user_id: UserId,
    pub audio: &'a [WhisperAudioSample],
    pub previous_transcript: &'a [i32],
    pub response_channel: oneshot::Sender<TranscriptionResponse>,
}
