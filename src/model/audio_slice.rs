use std::{
    cmp::{max, min},
    collections::VecDeque,
    num::Wrapping,
    time::{Duration, Instant, SystemTime},
};

use whisper_rs::WhisperToken;

use crate::events::audio::TranscriptionRequest;

use super::{
    constants::{
        AUDIO_TO_RECORD, AUDIO_TO_RECORD_SECONDS, AUTO_TRANSCRIPTION_PERIOD_MS, SILENT_TIMEOUT,
        TOKENS_TO_KEEP, USER_SILENCE_TIMEOUT,
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

fn discord_samples_to_whisper_samples(samples: usize) -> usize {
    samples / (BITRATE_CONVERSION_RATIO * DISCORD_AUDIO_CHANNELS)
}

fn samples_to_duration(num_samples: usize) -> Duration {
    Duration::from_millis((num_samples / WHISPER_SAMPLES_PER_MILLISECOND) as u64)
}

pub(crate) struct LastRequestInfo {
    pub start_time: SystemTime,
    pub original_duration: Duration,
    pub audio_trimmed_since_request: Duration,
    pub in_progress: bool,
    pub requested_at: SystemTime,
    pub final_request: bool,
}

impl LastRequestInfo {
    pub fn effective_duration(&self) -> Duration {
        self.original_duration - self.audio_trimmed_since_request
    }
}

pub(crate) struct AudioSlice {
    pub audio: Vec<WhisperAudioSample>,
    pub user_idle: bool,
    pub last_request: Option<LastRequestInfo>,
    pub slice_id: u64,
    pub start_time: Option<(DiscordRtcTimestamp, SystemTime)>,
    pub tentative_transcript_opt: Option<Transcription>,
    pub user_silent: bool,
    pub last_silent_time: Instant,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    /// Maximum length is TOKENS_TO_KEEP.
    pub last_tokens: std::collections::VecDeque<WhisperToken>,
}

impl AudioSlice {
    pub fn new(slice_id: u64) -> Self {
        Self {
            audio: Vec::with_capacity(WHISPER_AUDIO_BUFFER_SIZE),
            user_idle: false,
            last_request: None,
            slice_id,
            start_time: None,
            tentative_transcript_opt: None,
            user_silent: false,
            last_silent_time: Instant::now(),
            last_tokens: VecDeque::with_capacity(TOKENS_TO_KEEP),
        }
    }

    pub fn clear(&mut self) {
        eprintln!("{}: clearing audio slice", self.slice_id);
        self.audio.clear();
        self.user_idle = false;
        self.last_request = None;
        self.start_time = None;
        self.tentative_transcript_opt = None;
        self.user_silent = true;
    }

    pub fn set_silent(&mut self) {
        self.user_silent = true;
    }

    /// True if the given timestamp is within the bounds of this slice.
    /// Bounds are considered to begin with the start of the slice,
    /// and end within the given timeout of the end of the slice.
    /// An empty slice is considered to have no bounds, and will fit
    /// any timestamp.
    pub fn fits_within_this_slice(&self, rtc_timestamp: DiscordRtcTimestamp) -> bool {
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

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        // it seems like we get called with a single packet of silence right
        // after we are marked as silent.  Look for this and discard it.
        if self.user_silent && discord_audio.iter().all(|&x| x == 0) {
            // eprintln!("{}: ignoring silent audio", self.slice_id);
            return;
        }

        self.user_silent = false;

        let rtc_length = duration_to_rtc(&samples_to_duration(discord_audio.len()));

        if !self.fits_within_this_slice(rtc_timestamp + rtc_length) {
            // if the timestamp is not within the bounds of this slice,
            // then we need to create a new slice.
            eprintln!(
                "{}: trying to add audio to inactive slice, dropping audio",
                self.slice_id
            );
            return;
        }
        self.user_idle = false;

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

        if self.tentative_transcript_opt.is_some() {
            eprintln!("discarding tentative transcription");
            self.tentative_transcript_opt = None;
        }

        // update the last slice to point to the end of the buffer
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

    fn is_ready_for_transcription(&mut self) -> bool {
        if self.start_time.is_none() {
            return false;
        }

        if self.user_idle {
            // the slice is user_idle, but we haven't requested the full
            // buffer yet, so request it
            return true;
        }

        if let Some(last_request) = self.last_request.as_ref() {
            if last_request.in_progress {
                // we already have a pending request, so we don't need to
                // make another one
                return false;
            }
        }

        if self.user_silent {
            if self.last_silent_time.elapsed() > SILENT_TIMEOUT {
                // if the user is silent, then we need to request the full
                // buffer, even if no period shift has occurred
                // in this case we'll have two outstanding transcription
                // requests...
                self.last_silent_time = Instant::now();
                return true;
            } else {
                eprintln!("{}: user is silent, but not requesting transcription due to hysteresis prevention", self.slice_id);
            }
        }

        let current_period = self.buffer_duration().as_millis() / AUTO_TRANSCRIPTION_PERIOD_MS;
        let last_period;
        if let Some(last_request_info) = self.last_request.as_ref() {
            last_period =
                last_request_info.effective_duration().as_millis() / AUTO_TRANSCRIPTION_PERIOD_MS;
        } else {
            last_period = 0;
        }

        last_period != current_period
    }

    pub fn make_transcription_request(&mut self) -> Option<TranscriptionRequest> {
        if !self.is_ready_for_transcription() {
            return None;
        }
        if let Some((_, start_time)) = self.start_time {
            let duration = self.buffer_duration();
            eprintln!(
                "{}: requesting transcription for {} ms",
                self.slice_id,
                duration.as_millis()
            );
            let new_request = LastRequestInfo {
                start_time,
                audio_trimmed_since_request: Duration::ZERO,
                original_duration: self.buffer_duration(),
                in_progress: true,
                requested_at: SystemTime::now(),
                final_request: self.user_idle,
            };
            if let Some(last_request) = self.last_request.as_ref() {
                if last_request.start_time == new_request.start_time
                    && last_request.original_duration == new_request.original_duration
                {
                    eprintln!("{}: discarding duplicate request", self.slice_id);
                    // if this is our final request, make sure the last request
                    // has the final flag set
                    if new_request.final_request {
                        self.last_request.as_mut().unwrap().final_request = true;
                    }
                    return None;
                }
                if last_request.in_progress {
                    eprintln!("{}: discarding previous in-progress request", self.slice_id);
                }
            }
            self.last_request = Some(new_request);

            let buffer = self.audio.as_slice();

            return Some(TranscriptionRequest {
                audio_data: Vec::from(buffer),
                audio_duration: duration,
                previous_tokens: Vec::from(self.last_tokens.clone()),
                start_timestamp: start_time,
                user_id: self.slice_id,
            });
        }
        None
    }

    fn add_tokens(&mut self, message: &Transcription) {
        for segment in message.segments.iter() {
            for token in &segment.tokens_with_probability {
                self.last_tokens.push_back(token.token_id);
                if self.last_tokens.len() > TOKENS_TO_KEEP {
                    self.last_tokens.pop_front();
                }
            }
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

        // also update the last_request duration
        // call trim_audio to update the last_request
        if let Some(last_request) = self.last_request.as_mut() {
            last_request.audio_trimmed_since_request += *duration;
        }
    }

    pub fn handle_user_idle(&mut self) -> Option<Transcription> {
        self.user_idle = true;

        eprintln!(
            "finalizing slice with {} ms of audio",
            self.buffer_duration().as_millis()
        );

        self.tentative_transcript_opt.as_ref()?;

        let tentative_transcript = self.tentative_transcript_opt.take().unwrap();

        if !tentative_transcript.segments.is_empty() {
            eprintln!(
                "tentative description has {} segments, covering {} ms",
                tentative_transcript.segments.len(),
                tentative_transcript.audio_duration.as_millis(),
            );
        }

        // if we had a tentative transcription, return it.
        // We know that it's current, since if we had gathered
        // more audio, we would have discarded it.
        assert!(
            tentative_transcript.audio_duration == self.buffer_duration(),
            "tentative transcription duration != buffer duration",
        );
        self.clear();

        Some(tentative_transcript)
    }

    pub fn buffer_duration(&self) -> Duration {
        samples_to_duration(self.audio.len())
    }

    fn shortcut_tentative_transcripts(&self, message: &Transcription) -> bool {
        // figures out where the end of the message's audio is
        // in the current buffer.  Then checks for a period of
        // up to USER_SILENCE_TIMEOUT beyond that point.
        // If we reach the end of that period without seeing
        // any non-silence, we can return the tentative transcript
        // immediately, and this function will return true.
        // Otherwise, it returns false.
        let now = SystemTime::now();
        if message.start_timestamp + message.audio_duration + USER_SILENCE_TIMEOUT > now {
            // the future hasn't happened yet
            return false;
        }

        let end_idx = message.audio_duration.as_millis() as usize * WHISPER_SAMPLES_PER_MILLISECOND;
        let end_of_silence_interval =
            end_idx + USER_SILENCE_TIMEOUT.as_millis() as usize * WHISPER_SAMPLES_PER_MILLISECOND;

        if self.audio.len() < end_idx {
            eprintln!("{}: not enough audio for shortcut", self.slice_id);
            return false;
        }

        if (self.audio.len() < end_of_silence_interval) && !self.user_idle {
            eprintln!("{}: not enough audio for silence interval", self.slice_id);
            return false;
        }

        let end_of_search = min(self.audio.len(), end_of_silence_interval);
        // look through [end_idx, end_of_search) for non-silence
        let has_non_silence = self.audio[end_idx..end_of_search]
            .iter()
            .any(|&sample| sample != WhisperAudioSample::default());

        // if we found non-silence, we can't shortcut, otherwise we can
        !has_non_silence
    }

    pub fn handle_transcription_response(
        &mut self,
        message: &Transcription,
    ) -> Option<Transcription> {
        // if we had more than one request in flight, we need to
        // ignore all but the latest
        if let Some(last_request) = self.last_request.as_ref() {
            if last_request.start_time != message.start_timestamp {
                eprintln!(
                    "ignoring transcription response with start time {:?} (expected {:?})",
                    message.start_timestamp, last_request.start_time
                );
                return None;
            }
            if message.audio_duration != last_request.original_duration {
                eprintln!(
                    "ignoring transcription response with duration {:?} (expected {:?})",
                    message.audio_duration, last_request.original_duration
                );
                return None;
            }
        } else {
            // this can happen if we've cleared the buffer but had
            // an outstanding transcription request
            eprintln!("ignoring transcription response with no last request");
            return None;
        }
        self.last_request.as_mut().unwrap().in_progress = false;

        // todo: by this point more audio has been received, so we
        // have the data to know if the ends of our tentative segments
        // were followed by silence or not.  If not, we should discard
        // the tentative segments, but if they were, we can finalize
        // them now.
        // for now, just do this on a full-transcript basis, but it should
        // also be possible to do per-segment.
        if self.shortcut_tentative_transcripts(message) {
            eprintln!("fast-tracking tentative transcripts");
            self.tentative_transcript_opt = None;
            self.discard_audio(&message.audio_duration);
            return Some(message.clone());
        }

        // figure out how many segments have an end time that's more
        // than USER_SILENCE_TIMEOUT ago.  Those will be returned to
        // the caller in a Transcription.
        // The remainder, if any, will be kept in tentative_transcription,
        // but only if we haven't seen new audio since the response was generated.

        let end_time = if self.last_request.as_ref().unwrap().final_request {
            // if this is the final request, then we want to keep all
            // segments
            SystemTime::now() + Duration::from_secs(1000)
        } else {
            self.last_request.as_ref().unwrap().requested_at - USER_SILENCE_TIMEOUT
        };
        let (user_idle_transcript, tentative_transcript) =
            Transcription::split_at_end_time(message, end_time);
        // if self.user_idle {
        //     assert!(tentative_transcript.is_empty());
        // }
        assert_eq!(
            user_idle_transcript.audio_duration + tentative_transcript.audio_duration,
            message.audio_duration
        );
        // todo: check to see if the tentative transcription is accurate

        eprintln!(
            "have transcription: {} final segments ({} ms), {} tentative segments ({} ms)",
            user_idle_transcript.segments.len(),
            user_idle_transcript.audio_duration.as_millis(),
            tentative_transcript.segments.len(),
            tentative_transcript.audio_duration.as_millis(),
        );

        // eprintln!("user_idle transcription: '{}'", user_idle_transcript.text(),);
        // eprintln!("tentative transcription: '{}'", tentative_transcript.text(),);

        // with our user_idle transcription, we can now discard
        // the audio that was used to generate it.  Be sure to
        // only discard exactly as much audio as was represented
        // by the user_idle transcription, or the times will not line up.
        self.discard_audio(&user_idle_transcript.audio_duration);

        // if the remaining audio length is the same as the tentative
        // transcription, that means no new audio has arrived in the
        // meantime, so we can keep the tentative transcription.
        eprintln!(
            "tentative transcription with {} segments ({} ms): '{}'",
            tentative_transcript.segments.len(),
            tentative_transcript.audio_duration.as_millis(),
            tentative_transcript.text(),
        );

        if self.buffer_duration() == tentative_transcript.audio_duration {
            // eprintln!("keeping tentative transcription");
            self.tentative_transcript_opt = Some(tentative_transcript);
        } else {
            // eprintln!("discarding tentative transcription");
            self.tentative_transcript_opt = None;
        }

        if user_idle_transcript.is_empty() {
            None
        } else {
            self.add_tokens(&user_idle_transcript);
            Some(user_idle_transcript)
        }
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
