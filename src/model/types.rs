// we want to store 30 seconds of audio, 16-bit stereo PCM at 48kHz
// divided into 20ms chunks

use std::num::Wrapping;

pub const DISCORD_AUDIO_CHANNELS: usize = 2;

pub const DISCORD_SAMPLES_PER_SECOND: usize = 48000;
// pub const DISCORD_SAMPLES_PER_MILLISECOND: usize = DISCORD_SAMPLES_PER_SECOND / 1000;
// pub const DISCORD_PERIOD_PER_PACKET_GROUP_MS: usize = 20;
// pub const DISCORD_AUDIO_SAMPLES_PER_PACKET_GROUP_SINGLE_CHANNEL: usize =
//     DISCORD_SAMPLES_PER_MILLISECOND * DISCORD_PERIOD_PER_PACKET_GROUP_MS;

// the expected size of a single discord audio update, in samples
// pub const DISCORD_PACKET_GROUP_SIZE: usize =
//     DISCORD_AUDIO_SAMPLES_PER_PACKET_GROUP_SINGLE_CHANNEL * DISCORD_AUDIO_CHANNELS;

pub const AUDIO_TO_RECORD_SECONDS: usize = 30;

pub const WHISPER_SAMPLES_PER_SECOND: usize = 16000;
pub const WHISPER_SAMPLES_PER_MILLISECOND: usize = WHISPER_SAMPLES_PER_SECOND / 1000;

pub const RTC_CLOCK_SAMPLES_PER_SECOND: usize = 8000;

// being a whole number. If this is not the case, we'll need to
// do some more complicated resampling.
pub const BITRATE_CONVERSION_RATIO: usize = DISCORD_SAMPLES_PER_SECOND / WHISPER_SAMPLES_PER_SECOND;

// the total size of the buffer we'll use to store audio, in samples
pub const WHISPER_AUDIO_BUFFER_SIZE: usize = WHISPER_SAMPLES_PER_SECOND * AUDIO_TO_RECORD_SECONDS;

/// If an audio clip is less than this length, we'll ignore it.
pub const MIN_AUDIO_THRESHOLD_MS: u32 = 500;

/// keep this many tokens from previous transcriptions, and
/// use them to seed the next transcription.  This is per-user.
pub const TOKENS_TO_KEEP: usize = 1024;

pub const USER_SILENCE_TIMEOUT_MS: u64 = 2000;

pub const DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES: WhisperAudioSample =
    DISCORD_AUDIO_MAX_VALUE * DISCORD_AUDIO_CHANNELS as WhisperAudioSample;

pub type DiscordAudioSample = i16;
pub type DiscordRtcTimestamp = Wrapping<u32>;
pub type Ssrc = u32;
pub type UserId = u64;
pub type WhisperAudioSample = f32;
pub type WhisperToken = i32;

pub const DISCORD_AUDIO_MAX_VALUE: WhisperAudioSample =
    DiscordAudioSample::MAX as WhisperAudioSample;
