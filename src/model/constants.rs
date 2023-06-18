// we want to store 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks

use std::time::Duration;

pub(crate) const AUDIO_TO_RECORD_SECONDS: usize = 30;
pub(crate) const AUDIO_TO_RECORD: Duration = Duration::from_secs(AUDIO_TO_RECORD_SECONDS as u64);

pub(crate) const AUTO_TRANSCRIPTION_PERIOD_MS: u128 = 5000;

/// keep this many tokens from previous transcriptions, and
/// use them to seed the next transcription.  This is per-user.
pub(crate) const TOKENS_TO_KEEP: usize = 1024;

pub(crate) const USER_SILENCE_TIMEOUT_MS: usize = 1000;
pub(crate) const USER_SILENCE_TIMEOUT: Duration =
    Duration::from_millis(USER_SILENCE_TIMEOUT_MS as u64);

pub(crate) const DISCARD_USER_AUDIO_AFTER: Duration = Duration::from_secs(10 * 60);
