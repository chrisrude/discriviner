use std::{
    cmp::{max, min},
    num::Wrapping,
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use whisper_rs::WhisperToken;

use crate::model::{
    constants::{
        AUDIO_TO_RECORD, BITRATE_CONVERSION_RATIO, DISCORD_AUDIO_CHANNELS,
        DONT_EVEN_BOTHER_RMS_THRESHOLD, RTC_CLOCK_SAMPLES_PER_MILLISECOND,
        WHISPER_AUDIO_BUFFER_SIZE, WHISPER_SAMPLES_PER_MILLISECOND,
    },
    types::{
        DiscordAudioSample, DiscordRtcTimestamp, DiscordRtcTimestampInner, WhisperAudioSample,
    },
};

use super::events::TranscriptionRequest;

const DISCORD_AUDIO_MAX_VALUE: WhisperAudioSample = DiscordAudioSample::MAX as WhisperAudioSample;

pub(crate) const DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES: WhisperAudioSample =
    DISCORD_AUDIO_MAX_VALUE * DISCORD_AUDIO_CHANNELS as WhisperAudioSample;

fn duration_to_rtc(duration: &Duration) -> DiscordRtcTimestamp {
    let rtc_samples = duration.as_millis() * RTC_CLOCK_SAMPLES_PER_MILLISECOND;
    Wrapping(rtc_samples as DiscordRtcTimestampInner)
}

fn rtc_timestamp_to_index(ts1: &DiscordRtcTimestamp, ts2: &DiscordRtcTimestamp) -> usize {
    let delta = (ts2 - ts1).0 as usize;
    // we want the number of 16khz samples, so just multiply by 2.
    delta * WHISPER_SAMPLES_PER_MILLISECOND / RTC_CLOCK_SAMPLES_PER_MILLISECOND as usize
}

fn discord_samples_to_whisper_samples(samples: usize) -> usize {
    samples / (BITRATE_CONVERSION_RATIO * DISCORD_AUDIO_CHANNELS)
}

fn samples_to_duration(num_samples: usize) -> Duration {
    Duration::from_millis((num_samples / WHISPER_SAMPLES_PER_MILLISECOND) as u64)
}

fn duration_to_index(duration: &Duration) -> usize {
    duration.as_millis() as usize * WHISPER_SAMPLES_PER_MILLISECOND
}

pub fn rms_over_slice(audio_data: &[WhisperAudioSample]) -> f32 {
    // sum the squares of the samples in the range
    let sum_squares = audio_data.iter().map(|x| x * x).sum::<f32>();
    // divide by the number of samples, and take the square root
    (sum_squares / audio_data.len() as f32).sqrt()
}

pub(crate) struct AudioBuffer {
    pub audio: Vec<WhisperAudioSample>,
    pub slice_id: u64,
    pub start_time: Option<(DiscordRtcTimestamp, SystemTime)>,
}

impl AudioBuffer {
    pub fn new(slice_id: u64) -> Self {
        Self {
            audio: Vec::with_capacity(WHISPER_AUDIO_BUFFER_SIZE),
            slice_id,
            start_time: None,
        }
    }

    pub fn clear(&mut self) {
        self.audio.clear();
        self.start_time = None;
    }

    /// True if the given timestamp is within the bounds of this slice.
    /// Bounds are considered to begin with the start of the slice,
    /// and end with the start of the slice plus the length of the
    /// audio to record.
    /// An empty slice is considered to have no bounds, and will fit
    /// any timestamp.
    fn fits_within_this_slice(&self, rtc_timestamp: DiscordRtcTimestamp) -> bool {
        if self.start_time.is_none() {
            // this is a blank slice, so any timestamp fits
            return true;
        }
        let start_rtc = self.start_time.unwrap().0;
        let end = start_rtc + duration_to_rtc(&AUDIO_TO_RECORD);

        if start_rtc < end {
            rtc_timestamp >= start_rtc && rtc_timestamp < end
        } else {
            // if the slice wraps around, then we need to check
            // if the timestamp is either before the end or after
            // the start.
            rtc_timestamp < end || rtc_timestamp >= start_rtc
        }
    }

    pub fn make_transcription_request(
        &self,
        previous_tokens: Vec<WhisperToken>,
    ) -> Option<TranscriptionRequest> {
        self.start_time.map(|start_time| TranscriptionRequest {
            audio_bytes: self.get_bytes(),
            audio_duration: self.buffer_duration(),
            previous_tokens,
            start_timestamp: start_time.1,
            user_id: self.slice_id,
        })
    }

    /// True if the given audio can entirely fit within this slice.
    pub fn can_fit_audio(
        &self,
        rtc_timestamp: &DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) -> bool {
        let rtc_length = duration_to_rtc(&samples_to_duration(discord_audio.len()));

        if !self.fits_within_this_slice(rtc_timestamp + rtc_length) {
            // if the timestamp is not within the bounds of this slice,
            // then we need to create a new slice.
            eprintln!(
                "{}: trying to add audio to inactive slice, dropping audio",
                self.slice_id
            );
            return false;
        }
        true
    }

    pub fn remaining_capacity(&self) -> Duration {
        let remaining = WHISPER_AUDIO_BUFFER_SIZE - self.audio.len();
        samples_to_duration(remaining)
    }

    pub fn is_empty(&self) -> bool {
        self.audio.is_empty() && self.start_time.is_none()
    }

    /// Adds the given audio to the slice, resampling it from the
    /// discord format to the whisper format.
    /// If the slice is full, then the audio will be "silently" dropped.
    pub fn add_audio(
        &mut self,
        rtc_timestamp: &DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        // if audio is entirely silent, then don't add it
        if discord_audio
            .iter()
            .all(|&x| x == DiscordAudioSample::default())
        {
            return;
        }
        if !self.can_fit_audio(rtc_timestamp, discord_audio) {
            eprintln!("{}: buffer full, dropping audio", self.slice_id);
            return;
        }

        let start_index;
        if let Some((start_rtc, _)) = self.start_time.as_ref() {
            start_index = rtc_timestamp_to_index(start_rtc, rtc_timestamp);
        } else {
            // this is the first audio for the slice, so we need to set
            // the start time
            self.start_time = Some((*rtc_timestamp, SystemTime::now()));
            start_index = 0;
        }

        self.resample_audio_from_discord_to_whisper(start_index, discord_audio);
    }

    /// Transcode the audio into the given location of the buffer,
    /// converting it from Discord's format (48khz stereo PCM16)
    /// to Whisper's format (16khz mono f32).
    ///
    /// This handles several cases:
    ///  - a single allocation for the new audio at the end of the buffer
    ///  - also, inserting silence if the new audio is not contiguous with
    ///    the previous audio
    ///  - doing it in a way that we can also backfill audio if we get
    ///    packets out-of-order
    ///
    fn resample_audio_from_discord_to_whisper(
        &mut self,
        start_index: usize,
        discord_audio: &[DiscordAudioSample],
    ) {
        let end_index = start_index + discord_samples_to_whisper_samples(discord_audio.len());
        let buffer_len = max(self.audio.len(), end_index);

        self.audio.resize(buffer_len, WhisperAudioSample::default());

        let dest_buf = &mut self.audio[start_index..end_index];

        for (i, samples) in discord_audio
            .chunks_exact(BITRATE_CONVERSION_RATIO * DISCORD_AUDIO_CHANNELS)
            .enumerate()
        {
            // sum the channel data, and divide by the max value possible to
            // get a value between -1.0 and 1.0
            dest_buf[i] = samples
                .iter()
                .take(DISCORD_AUDIO_CHANNELS)
                .map(|x| *x as WhisperAudioSample)
                .sum::<WhisperAudioSample>()
                / DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES;
        }
    }

    /// Returns the current audio buffer as a Bytes reference.
    /// This does not copy the audio data, so it is only valid
    /// for the lifetime of the audio slice.
    pub fn get_bytes(&self) -> Bytes {
        let buffer = self.audio.as_slice();
        let buffer_len_bytes = std::mem::size_of_val(buffer);
        let byte_data =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, buffer_len_bytes) };
        Bytes::from(byte_data)
    }

    /// Discards the amount of audio specified by the duration
    /// from the start of the buffer, shuffling the remaining
    /// audio to the start of the buffer.  Any indexes and
    /// timestamps are adjusted accordingly.
    pub fn discard_audio(&mut self, duration: &Duration) {
        let discard_idx = duration.as_millis() as usize * WHISPER_SAMPLES_PER_MILLISECOND;

        if duration.is_zero() {
            return;
        }

        eprintln!(
            "discarding {} ms of audio from {} ms buffer",
            duration.as_millis(),
            self.buffer_duration().as_millis()
        );

        if discard_idx >= self.audio.len() {
            // discard as much as we have, so just clear the buffer
            self.clear();
            return;
        }

        // eliminate this many samples from the start of the buffer
        self.audio.drain(0..discard_idx);

        // update the start timestamp
        if let Some((start_rtc, start_system)) = self.start_time {
            self.start_time = Some((
                start_rtc + duration_to_rtc(duration),
                start_system + *duration,
            ));
        }
    }

    /// Returns the length of the audio stored in the buffer,
    /// in units of time.
    pub fn buffer_duration(&self) -> Duration {
        samples_to_duration(self.audio.len())
    }

    pub fn rms_over_interval(&self, start: &Duration, interval_length: &Duration) -> f32 {
        let (idx_start, idx_end) = self.clamped_range(start, interval_length);
        rms_over_slice(&self.audio[idx_start..idx_end])
    }

    /// Returns true only if the period from [start, start + interval_length] contains
    /// only silence.  If none of the period is in the buffer, then this returns false.
    /// If only part of the period is in the buffer, and all of that period is silence,
    /// then this returns true.
    /// The one exception is for when the start period is one sample beyond the
    /// end of the buffer, in which case this returns true.
    pub fn is_interval_silent(&self, start: &Duration, interval_length: &Duration) -> bool {
        // figures out where the end of the message's audio is
        // in the current buffer.  Then checks for a period of
        // up to USER_SILENCE_TIMEOUT beyond that point.
        // If we reach the end of that period without seeing
        // any non-silence, we can return the tentative transcript
        // immediately, and this function will return true.
        // Otherwise, it returns false.

        let (idx_start, idx_end) = self.clamped_range(start, interval_length);
        let slice = &self.audio[idx_start..idx_end];
        slice
            .iter()
            .all(|&sample| sample == WhisperAudioSample::default())
            || (rms_over_slice(slice) < DONT_EVEN_BOTHER_RMS_THRESHOLD)
    }

    fn clamped_range(&self, start: &Duration, interval_length: &Duration) -> (usize, usize) {
        let idx_start = duration_to_index(start);
        let idx_end = idx_start + duration_to_index(interval_length);

        let idx_start = min(idx_start, self.audio.len());
        let idx_end = min(idx_end, self.audio.len());

        (idx_start, idx_end)
    }
}

#[cfg(test)]
mod tests {
    use crate::model::constants::{AUDIO_TO_RECORD_SECONDS, DISCORD_SAMPLES_PER_SECOND};

    use super::*;
    use std::time::Duration;

    #[test]
    fn test_discard_audio() {
        let mut slice = AudioBuffer::new(123);
        slice.start_time = Some((
            Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            SystemTime::now(),
        ));
        slice.audio = vec![0.0; 1000 * WHISPER_SAMPLES_PER_MILLISECOND];
        assert_eq!(slice.buffer_duration(), Duration::from_millis(1000));

        slice.discard_audio(&Duration::from_millis(500));

        assert_eq!(slice.buffer_duration(), Duration::from_millis(500));
        assert_eq!(slice.audio.len(), 500 * WHISPER_SAMPLES_PER_MILLISECOND);
        let time = slice.start_time.unwrap().0;
        assert_eq!(time.0, 1500 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);
    }

    const DISCORD_SAMPLES_PER_MILLISECOND: usize = DISCORD_SAMPLES_PER_SECOND / 1000;
    #[test]
    fn test_add_audio() {
        let mut slice = AudioBuffer::new(234);
        slice.start_time = Some((
            Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            SystemTime::now(),
        ));

        // check the buffer capacity at the top.  At the end it should
        // still the same.
        let original_capacity = slice.audio.capacity();

        slice.audio = vec![0.0; 1000 * WHISPER_SAMPLES_PER_MILLISECOND];
        assert_eq!(slice.buffer_duration(), Duration::from_millis(1000));

        slice.add_audio(
            &Wrapping(2000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            &vec![1; 500 * DISCORD_SAMPLES_PER_MILLISECOND * DISCORD_AUDIO_CHANNELS],
        );

        assert_eq!(slice.buffer_duration(), Duration::from_millis(1500));
        assert_eq!(slice.audio.len(), 1500 * WHISPER_SAMPLES_PER_MILLISECOND);
        let time = slice.start_time.unwrap().0;
        assert_eq!(time.0, 1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        slice.add_audio(
            &Wrapping(4000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            &vec![1; 500 * DISCORD_SAMPLES_PER_MILLISECOND * DISCORD_AUDIO_CHANNELS],
        );

        assert_eq!(slice.buffer_duration(), Duration::from_millis(3500));
        assert_eq!(slice.audio.len(), 3500 * WHISPER_SAMPLES_PER_MILLISECOND);
        let time = slice.start_time.unwrap().0;
        assert_eq!(time.0, 1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        let max_acceptable_rtc = (((1 + AUDIO_TO_RECORD_SECONDS) * 1000)
            * RTC_CLOCK_SAMPLES_PER_MILLISECOND as usize) as u32
            - 1;
        // don't add audio that's too far in the future
        slice.add_audio(
            &Wrapping(max_acceptable_rtc),
            &vec![1; 500 * DISCORD_SAMPLES_PER_MILLISECOND * DISCORD_AUDIO_CHANNELS],
        );

        assert_eq!(slice.buffer_duration(), Duration::from_millis(3500));
        assert_eq!(slice.audio.len(), 3500 * WHISPER_SAMPLES_PER_MILLISECOND);
        let time = slice.start_time.unwrap().0;
        assert_eq!(time.0, 1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        assert!(
            slice.fits_within_this_slice(Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32))
        );

        assert!(
            !slice.fits_within_this_slice(Wrapping(999 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32))
        );

        assert!(slice.fits_within_this_slice(Wrapping(max_acceptable_rtc)));

        assert!(!slice.fits_within_this_slice(Wrapping(max_acceptable_rtc + 1)));

        // make sure the buffer didn't grow
        assert!(slice.audio.capacity() <= original_capacity);
    }

    #[test]
    fn test_is_interval_silent() {
        let mut slice = AudioBuffer::new(345);
        let start_rtc = Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        slice.start_time = Some((start_rtc, SystemTime::now()));

        const STEREO_SAMPLES_PER_SECOND: usize =
            DISCORD_SAMPLES_PER_SECOND * DISCORD_AUDIO_CHANNELS;

        const SILENT_DISCORD_AUDIO: [DiscordAudioSample; STEREO_SAMPLES_PER_SECOND] =
            [0; STEREO_SAMPLES_PER_SECOND];

        const NOISY_DISCORD_AUDIO: [DiscordAudioSample; STEREO_SAMPLES_PER_SECOND] =
            [DiscordAudioSample::MAX / 2; STEREO_SAMPLES_PER_SECOND];

        const ONE_SECOND_RTC: Wrapping<u32> =
            Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        const ONE_SECOND: Duration = Duration::from_secs(1);

        const NEAR_SILENT_DISCORD_AUDIO: [DiscordAudioSample; STEREO_SAMPLES_PER_SECOND] =
            [1 as DiscordAudioSample; STEREO_SAMPLES_PER_SECOND];

        // seconds [0,1) will be silent, since we'll auto-pad with silence
        // seconds [1,2) are silent
        assert_eq!(slice.buffer_duration(), Duration::ZERO);

        // silent audio should not be added, except as padding
        slice.add_audio(&(start_rtc + ONE_SECOND_RTC), &SILENT_DISCORD_AUDIO);
        assert_eq!(slice.buffer_duration(), Duration::ZERO);

        // seconds [2,3) are noisy
        slice.add_audio(
            &(start_rtc + ONE_SECOND_RTC + ONE_SECOND_RTC),
            &NOISY_DISCORD_AUDIO,
        );
        assert_eq!(slice.buffer_duration(), 3 * ONE_SECOND);

        // seconds [3,4) are silent again
        slice.add_audio(
            &(start_rtc + ONE_SECOND_RTC + ONE_SECOND_RTC + ONE_SECOND_RTC),
            &SILENT_DISCORD_AUDIO,
        );
        assert_eq!(slice.buffer_duration(), 3 * ONE_SECOND);

        // seconds [4,5) are nearly silent, and should be considered silent in our test
        slice.add_audio(
            &(start_rtc + ONE_SECOND_RTC + ONE_SECOND_RTC + ONE_SECOND_RTC + ONE_SECOND_RTC),
            &NEAR_SILENT_DISCORD_AUDIO,
        );

        assert!(slice.is_interval_silent(&Duration::ZERO, &ONE_SECOND));
        assert!(slice.is_interval_silent(&(ONE_SECOND / 2), &ONE_SECOND));
        assert!(slice.is_interval_silent(&(1 * ONE_SECOND), &ONE_SECOND));
        //assert!(!slice.is_interval_silent(&(3 * ONE_SECOND / 2), &ONE_SECOND));
        assert!(!slice.is_interval_silent(&(2 * ONE_SECOND), &ONE_SECOND));
        assert!(slice.is_interval_silent(&(3 * ONE_SECOND), &ONE_SECOND));

        assert!(slice.is_interval_silent(&(4 * ONE_SECOND), &ONE_SECOND));

        let duration_buffer_len_minus_one = samples_to_duration(slice.audio.len() - 1);
        assert!(slice.is_interval_silent(&duration_buffer_len_minus_one, &ONE_SECOND));

        let duration_buffer_len = samples_to_duration(slice.audio.len());
        assert!(slice.is_interval_silent(&duration_buffer_len, &ONE_SECOND));

        let duration_buffer_len_plus_one = samples_to_duration(slice.audio.len() + 1);
        assert!(slice.is_interval_silent(&duration_buffer_len_plus_one, &ONE_SECOND));
    }
}
