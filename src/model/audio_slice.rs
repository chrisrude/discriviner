use std::{
    cmp::max,
    num::Wrapping,
    time::{Duration, SystemTime},
};

use crate::model::constants::WHISPER_SAMPLES_PER_MILLISECOND;

use super::{
    constants::{AUDIO_TO_RECORD, AUDIO_TO_RECORD_SECONDS, WHISPER_SAMPLES_PER_SECOND},
    types::{
        self, DiscordAudioSample, DiscordRtcTimestamp, DiscordRtcTimestampInner, WhisperAudioSample,
    },
};

const DISCORD_AUDIO_CHANNELS: usize = 2;
const DISCORD_SAMPLES_PER_SECOND: usize = 48000;

// The RTC timestamp uses an 48khz clock.
const RTC_CLOCK_SAMPLES_PER_MILLISECOND: u128 = 48;

// being a whole number. If this is not the case, we'll need to
// do some more complicated resampling.
const BITRATE_CONVERSION_RATIO: usize = DISCORD_SAMPLES_PER_SECOND / WHISPER_SAMPLES_PER_SECOND;

// the total size of the buffer we'll use to store audio, in samples
const WHISPER_AUDIO_BUFFER_SIZE: usize = WHISPER_SAMPLES_PER_SECOND * AUDIO_TO_RECORD_SECONDS;

const DISCORD_AUDIO_MAX_VALUE: WhisperAudioSample = DiscordAudioSample::MAX as WhisperAudioSample;

pub(crate) const DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES: WhisperAudioSample =
    DISCORD_AUDIO_MAX_VALUE * DISCORD_AUDIO_CHANNELS as WhisperAudioSample;

fn duration_to_rtc(duration: &Duration) -> DiscordRtcTimestamp {
    let rtc_samples = duration.as_millis() * RTC_CLOCK_SAMPLES_PER_MILLISECOND;
    Wrapping(rtc_samples as DiscordRtcTimestampInner)
}

fn rtc_timestamp_to_index(ts1: DiscordRtcTimestamp, ts2: DiscordRtcTimestamp) -> usize {
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

pub(crate) struct AudioSlice {
    pub audio: Vec<WhisperAudioSample>,
    pub slice_id: u64,
    pub start_time: Option<(DiscordRtcTimestamp, SystemTime)>,
}

impl AudioSlice {
    pub fn new(slice_id: u64) -> Self {
        Self {
            audio: Vec::with_capacity(WHISPER_AUDIO_BUFFER_SIZE),
            slice_id,
            start_time: None,
        }
    }

    pub fn clear(&mut self) {
        eprintln!("{}: clearing audio slice", self.slice_id);
        self.audio.clear();
        self.start_time = None;
    }

    /// True if the given timestamp is within the bounds of this slice.
    /// Bounds are considered to begin with the start of the slice,
    /// and end within the given timeout of the end of the slice.
    /// An empty slice is considered to have no bounds, and will fit
    /// any timestamp.
    fn fits_within_this_slice(&self, rtc_timestamp: DiscordRtcTimestamp) -> bool {
        if let Some((start_rtc, _)) = self.start_time {
            // add end of buffer
            // note: this will ignore the size of the audio we're looking to
            // add, but that's ok
            let end = start_rtc + duration_to_rtc(&AUDIO_TO_RECORD);

            let result = if start_rtc < end {
                rtc_timestamp >= start_rtc && rtc_timestamp < end
            } else {
                // if the slice wraps around, then we need to check
                // if the timestamp is either before the end or after
                // the start.
                rtc_timestamp < end || rtc_timestamp >= start_rtc
            };

            if !result {
                eprintln!(
                    "{}: timestamp {} does not fit within slice.  start={:?} end={:?}",
                    self.slice_id, rtc_timestamp, self.start_time, end,
                )
            }
            result
        } else {
            // this is a blank slice, so any timestamp fits
            true
        }
    }

    pub fn can_fit_audio(
        &self,
        rtc_timestamp: DiscordRtcTimestamp,
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

    pub fn is_empty(&self) -> bool {
        self.start_time.is_none() && self.audio.is_empty()
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        if !self.can_fit_audio(rtc_timestamp, discord_audio) {
            eprintln!("{}: buffer full, dropping audio", self.slice_id);
            return;
        }

        let start_index;
        if let Some((start_rtc, _)) = self.start_time {
            start_index = rtc_timestamp_to_index(start_rtc, rtc_timestamp);
        } else {
            // this is the first audio for the slice, so we need to set
            // the start time
            self.start_time = Some((rtc_timestamp, SystemTime::now()));
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
                .map(|x| *x as types::WhisperAudioSample)
                .sum::<types::WhisperAudioSample>()
                / DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES;
        }
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

    pub fn buffer_duration(&self) -> Duration {
        samples_to_duration(self.audio.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_discard_audio() {
        let mut slice = AudioSlice::new(123);
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
        let mut slice = AudioSlice::new(234);
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
            Wrapping(2000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            &vec![1; 500 * DISCORD_SAMPLES_PER_MILLISECOND * DISCORD_AUDIO_CHANNELS],
        );

        assert_eq!(slice.buffer_duration(), Duration::from_millis(1500));
        assert_eq!(slice.audio.len(), 1500 * WHISPER_SAMPLES_PER_MILLISECOND);
        let time = slice.start_time.unwrap().0;
        assert_eq!(time.0, 1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);

        slice.add_audio(
            Wrapping(4000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
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
            Wrapping(max_acceptable_rtc),
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
}
