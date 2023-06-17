// we want to store 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks

use std::{num::Wrapping, time::Duration};

pub const DISCORD_AUDIO_CHANNELS: usize = 2;
pub const DISCORD_SAMPLES_PER_SECOND: usize = 48000;

const WHISPER_SAMPLES_PER_SECOND: usize = 16000;
pub const WHISPER_SAMPLES_PER_MILLISECOND: usize = WHISPER_SAMPLES_PER_SECOND / 1000;

// The RTC timestamp uses an 8khz clock.
// todo: verify
const RTC_CLOCK_SAMPLES_PER_SECOND: u128 = 8000;
pub const RTC_CLOCK_SAMPLES_PER_MILLISECOND: u128 = RTC_CLOCK_SAMPLES_PER_SECOND / 1000;

// being a whole number. If this is not the case, we'll need to
// do some more complicated resampling.
pub const BITRATE_CONVERSION_RATIO: usize = DISCORD_SAMPLES_PER_SECOND / WHISPER_SAMPLES_PER_SECOND;

const AUDIO_TO_RECORD_SECONDS: usize = 30;
// the total size of the buffer we'll use to store audio, in samples
pub const WHISPER_AUDIO_BUFFER_SIZE: usize = WHISPER_SAMPLES_PER_SECOND * AUDIO_TO_RECORD_SECONDS;

pub const AUTO_TRANSCRIPTION_PERIOD_MS: usize = 5000;
pub const AUTO_TRANSCRIPTION_PERIOD: Duration =
    Duration::from_millis(AUTO_TRANSCRIPTION_PERIOD_MS as u64);

/// keep this many tokens from previous transcriptions, and
/// use them to seed the next transcription.  This is per-user.
pub const TOKENS_TO_KEEP: usize = 1024;

pub const USER_SILENCE_TIMEOUT_MS: usize = 2000;
pub const USER_SILENCE_TIMEOUT: Duration = Duration::from_millis(USER_SILENCE_TIMEOUT_MS as u64);

// add a slightly less strict timeout, as there can be some variance
// between the processing of the different tasks.  We'll generate timeout
// events using the stricter timeout, but allow them to be applied even
// if the lesser timeout fits.
pub const USER_SILENCE_TIMEOUT_LOOSE: Duration =
    Duration::from_millis(USER_SILENCE_TIMEOUT_MS as u64 - 100);

pub const DISCARD_USER_AUDIO_AFTER: Duration = Duration::from_secs(10 * 60);

pub type DiscordAudioSample = i16;
pub type DiscordRtcTimestampInner = u32;
pub type DiscordRtcTimestamp = Wrapping<DiscordRtcTimestampInner>;
pub type Ssrc = u32;
pub type UserId = u64;
pub type WhisperAudioSample = f32;
pub type WhisperToken = i32;

// this is a percentage, so it's between 0 and 100
// use this instead of a float to allow our API types to
// implement Eq, and thus be used as keys in a HashMap
pub type WhisperTokenProbabilityPercentage = u32;

const DISCORD_AUDIO_MAX_VALUE: WhisperAudioSample = DiscordAudioSample::MAX as WhisperAudioSample;

pub const DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES: WhisperAudioSample =
    DISCORD_AUDIO_MAX_VALUE * DISCORD_AUDIO_CHANNELS as WhisperAudioSample;
