use std::{
    iter,
    num::Wrapping,
    time::{Duration, SystemTime},
};

use bytes::Bytes;

use super::{
    constants::{
        AUDIO_TO_RECORD_SECONDS, AUTO_TRANSCRIPTION_PERIOD_MS, USER_SILENCE_TIMEOUT,
        USER_SILENCE_TIMEOUT_LOOSE,
    },
    types::{
        self, DiscordAudioSample, DiscordRtcTimestamp, DiscordRtcTimestampInner, Transcription,
        WhisperAudioSample,
    },
};

const DISCORD_AUDIO_CHANNELS: usize = 2;
const DISCORD_SAMPLES_PER_SECOND: usize = 48000;

const WHISPER_SAMPLES_PER_SECOND: usize = 16000;
const WHISPER_SAMPLES_PER_MILLISECOND: usize = 16;

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

fn samples_to_duration(num_samples: usize) -> u64 {
    (num_samples / WHISPER_SAMPLES_PER_MILLISECOND) as u64
}

pub(crate) struct AudioSlice {
    pub audio: Vec<WhisperAudioSample>,
    pub finalized: bool,
    pub last_request: Option<Duration>,
    pub start_time: Option<(DiscordRtcTimestamp, SystemTime)>,
    pub tentative_transcript_opt: Option<Transcription>,
}

impl AudioSlice {
    pub fn new() -> Self {
        Self {
            audio: Vec::with_capacity(WHISPER_AUDIO_BUFFER_SIZE),
            finalized: false,
            last_request: None,
            start_time: None,
            tentative_transcript_opt: None,
        }
    }

    pub fn clear(&mut self) {
        eprintln!("clearing audio slice");
        self.audio.clear();
        self.finalized = false;
        self.last_request = None;
        self.start_time = None;
        self.tentative_transcript_opt = None;
    }

    /// True if the given timestamp is within the bounds of this slice.
    /// Bounds are considered to begin with the start of the slice,
    /// and end within the given timeout of the end of the slice.
    /// An empty slice is considered to have no bounds, and will fit
    /// any timestamp.
    pub fn fits_within_this_slice(&self, rtc_timestamp: DiscordRtcTimestamp) -> bool {
        if self.finalized {
            // if the slice is finalized, then it can take no more audio
            return false;
        }
        if let Some((start_rtc, _)) = self.start_time {
            let current_end = start_rtc + duration_to_rtc(&self.buffer_duration());

            // add end_timeout to end
            let timeout = duration_to_rtc(&USER_SILENCE_TIMEOUT);
            let end = current_end + timeout;

            if start_rtc < end {
                rtc_timestamp >= start_rtc && rtc_timestamp < end
            } else {
                // if the slice wraps around, then we need to check
                // if the timestamp is either before the end or after
                // the start.
                rtc_timestamp < end || rtc_timestamp >= start_rtc
            }
        } else {
            // this is a blank slice, so any timestamp fits
            true
        }
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        if !self.fits_within_this_slice(rtc_timestamp) {
            // if the timestamp is not within the bounds of this slice,
            // then we need to create a new slice.
            if self.finalized {
                eprintln!("audio buffer overflow, dropping audio");
                return;
            }
            eprintln!("trying to add audio to inactive slice, dropping audio");
            return;
        }
        // calculate the time difference between the end of the last
        // slice and the start of this one.  If it is more than
        let padding;
        if let Some((start_rtc, _)) = self.start_time {
            // padding is the difference between the current end time
            // and the start of the new audio
            eprintln!(
                "rtc timestamp: {}... start {}",
                rtc_timestamp,
                self.start_time.unwrap().0
            );
            let rtc_idx = rtc_timestamp_to_index(start_rtc, rtc_timestamp);
            eprintln!("rtc index: {}", rtc_idx);
            eprintln!("audio len: {}", self.audio.len());
            padding = rtc_idx - self.audio.len();
        } else {
            // this is the first audio for the slice, so we need to set
            // the start time
            self.start_time = Some((rtc_timestamp, SystemTime::now()));
            padding = 0;
        }

        // if there's a gap between the end of our current
        // audio and the start of the new audio, fill it
        // with silence.
        if padding > 0 {
            eprintln!(
                "padding audio with {} samples, total of {} ms",
                padding,
                samples_to_duration(padding)
            );
            eprintln!(
                "rtc timestamp: {}... start {}",
                rtc_timestamp,
                self.start_time.unwrap().0
            );
            self.audio
                .extend(iter::repeat(WhisperAudioSample::default()).take(padding));
        }

        if self.tentative_transcript_opt.is_some() {
            eprintln!("discarding tentative transcription");
        }

        // since more audio has come in, discard the tentative transcription
        self.tentative_transcript_opt = None;

        self.resample_audio_from_discord_to_whisper(discord_audio);

        // update the last slice to point to the end of the buffer
    }

    fn resample_audio_from_discord_to_whisper(&mut self, audio: &[types::DiscordAudioSample]) {
        for samples in audio.chunks_exact(BITRATE_CONVERSION_RATIO * DISCORD_AUDIO_CHANNELS) {
            // sum the channel data, and divide by the max value possible to
            // get a value between -1.0 and 1.0
            self.audio.push(
                samples
                    .iter()
                    .take(DISCORD_AUDIO_CHANNELS)
                    .map(|x| *x as types::WhisperAudioSample)
                    .sum::<types::WhisperAudioSample>()
                    / DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES,
            );
        }
    }

    fn is_ready_for_transcription(&self, user_silent: bool) -> bool {
        if self.start_time.is_none() {
            return false;
        }

        let last_period;
        if let Some(last) = self.last_request {
            if last == self.buffer_duration() {
                // if the last request was for the entire buffer, then
                // we don't need to request again
                return false;
            }
            last_period = last.as_millis() / AUTO_TRANSCRIPTION_PERIOD_MS;
        } else {
            last_period = 0;
        }

        if self.finalized {
            // the slice is finalized, but we haven't requested the full
            // buffer yet, so request it
            return true;
        }

        if user_silent {
            // if the user is silent, then we need to request the full
            // buffer, even if no period shift has occurred
            return true;
        }

        let current_period = self.buffer_duration().as_millis() / AUTO_TRANSCRIPTION_PERIOD_MS;

        last_period != current_period
    }

    pub fn make_transcription_request(
        &mut self,
        user_silent: bool,
    ) -> Option<(Bytes, Duration, SystemTime)> {
        if !self.is_ready_for_transcription(user_silent) {
            return None;
        }
        if let Some((_, start_time)) = self.start_time {
            let buffer = self.audio.as_slice();
            let buffer_len_bytes = std::mem::size_of_val(buffer);
            let byte_data = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr() as *const u8, buffer_len_bytes)
            };

            let duration = self.buffer_duration();
            eprintln!("requesting transcription for {} ms", duration.as_millis());
            self.last_request = Some(duration);
            return Some((Bytes::from(byte_data), duration, start_time));
        }
        None
    }

    /// Discards the amount of audio specified by the duration
    /// from the start of the buffer, shuffling the remaining
    /// audio to the start of the buffer.  Any indexes and
    /// timestamps are adjusted accordingly.
    pub fn discard_audio(&mut self, duration: &Duration) {
        let discard_idx = duration.as_millis() as usize * WHISPER_SAMPLES_PER_MILLISECOND;

        eprintln!(
            "discarding {} ms of audio from {} ms buffer",
            duration.as_millis(),
            self.buffer_duration().as_millis()
        );

        if discard_idx > self.audio.len() {
            // discard more than we have, so just clear the buffer
            self.clear();
            return;
        }

        // eliminate this many samples from the start of the buffer
        self.audio.drain(0..discard_idx);

        // update the start timestamp
        if let Some((start_rtc, start_system)) = self.start_time {
            self.start_time = Some((
                start_rtc - duration_to_rtc(duration),
                start_system + *duration,
            ));
        }

        // also update the last_request duration
        if let Some(last_request) = self.last_request {
            self.last_request = Some(last_request - *duration);
        }
    }

    pub fn finalize(&mut self) -> Option<Transcription> {
        self.finalized = true;

        eprintln!(
            "finalizing slice with {} ms of audio",
            self.buffer_duration().as_millis()
        );

        if self.tentative_transcript_opt.is_none() {
            // if we don't have a tentative transcription, then
            // we can't return anything
            eprintln!("no tentative transcription in finalize, returning None");
            return None;
        }

        let tentative_transcript = self.tentative_transcript_opt.take().unwrap();

        eprintln!(
            "tentative description has {} segments, covering {} ms",
            tentative_transcript.segments.len(),
            tentative_transcript.audio_duration.as_millis(),
        );

        // if we had a tentative transcription, return it.
        // We know that it's current, since if we had gathered
        // more audio, we would have discarded it.
        assert!(tentative_transcript.audio_duration == self.buffer_duration());
        self.clear();

        Some(tentative_transcript)
    }

    pub fn buffer_duration(&self) -> Duration {
        Duration::from_millis(samples_to_duration(self.audio.len()))
    }

    pub fn finalize_timestamp(&self) -> Option<SystemTime> {
        if let Some((_, start_time)) = self.start_time {
            Some(start_time + self.buffer_duration() + USER_SILENCE_TIMEOUT_LOOSE)
        } else {
            None
        }
    }

    pub fn handle_transcription_response(
        &mut self,
        message: &Transcription,
    ) -> Option<Transcription> {
        // figure out how many segments have an end time that's more
        // than USER_SILENCE_TIMEOUT ago.  Those will be returned to
        // the caller in a Transcription.
        // The remainder, if any, will be kept in tentative_transcription,
        // but only if we haven't seen new audio since the response was generated.

        if let Some(end_time) = self.finalize_timestamp() {
            let (finalized_transcript, tentative_transcript) =
                Transcription::split_at_end_time(message, end_time);
            if self.finalized {
                assert!(tentative_transcript.is_empty());
            }
            assert_eq!(
                finalized_transcript.audio_duration + tentative_transcript.audio_duration,
                message.audio_duration
            );

            eprintln!(
                "have transcription: {} final segments ({} ms), {} tentative segments ({} ms)",
                finalized_transcript.segments.len(),
                finalized_transcript.audio_duration.as_millis(),
                tentative_transcript.segments.len(),
                tentative_transcript.audio_duration.as_millis(),
            );
            eprintln!("finalized transcription: '{}'", finalized_transcript.text(),);
            eprintln!("tentative transcription: '{}'", tentative_transcript.text(),);

            // with our finalized transcription, we can now discard
            // the audio that was used to generate it.  Be sure to
            // only discard exactly as much audio as was represented
            // by the finalized transcription, or the times will not line up.
            self.discard_audio(&finalized_transcript.audio_duration);

            // if the remaining audio length is the same as the tentative
            // transcription, that means no new audio has arrived in the
            // meantime, so we can keep the tentative transcription.
            if self.buffer_duration() == tentative_transcript.audio_duration {
                eprintln!("keeping tentative transcription");
                self.tentative_transcript_opt = Some(tentative_transcript);
            } else {
                eprintln!("discarding tentative transcription");
                self.tentative_transcript_opt = None;
            }

            Some(finalized_transcript)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_discard_audio() {
        let mut slice = AudioSlice::new();
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
        assert_eq!(time.0, 500 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32);
    }

    const DISCORD_SAMPLES_PER_MILLISECOND: usize = DISCORD_SAMPLES_PER_SECOND / 1000;
    #[test]
    fn test_add_audio() {
        let mut slice = AudioSlice::new();
        slice.start_time = Some((
            Wrapping(1000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
            SystemTime::now(),
        ));
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

        // don't add audio that's too far in the future
        slice.add_audio(
            Wrapping(8000 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32),
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

        assert!(
            slice.fits_within_this_slice(Wrapping(6499 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32))
        );

        assert!(!slice
            .fits_within_this_slice(Wrapping(6500 * RTC_CLOCK_SAMPLES_PER_MILLISECOND as u32)));
    }
}
