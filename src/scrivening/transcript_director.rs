use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime},
};

use whisper_rs::WhisperToken;

use crate::{
    audio::{audio_buffer::AudioBuffer, events::TranscriptionRequest},
    model::{
        constants::{
            AUTO_TRANSCRIPTION_PERIOD_MS, MAX_LAG, SILENT_TIMEOUT, TOKENS_TO_KEEP,
            USER_SILENCE_TIMEOUT,
        },
        types::{DiscordAudioSample, DiscordRtcTimestamp, Transcription},
    },
};

/// Determines when transcriptions should be attempted,
/// and sends the results to the API.

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

pub(crate) struct TranscriptDirector {
    pub user_idle: bool,
    pub last_request: Option<LastRequestInfo>,
    pub tentative_transcript_opt: Option<Transcription>,
    pub user_silent: bool,
    pub last_silent_time: Instant,
    pub slice_id: u64,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    /// Maximum length is TOKENS_TO_KEEP.
    pub last_tokens: std::collections::VecDeque<WhisperToken>,

    pub slice: AudioBuffer,
}

impl TranscriptDirector {
    pub fn new(slice_id: u64) -> Self {
        Self {
            user_idle: false,
            last_request: None,
            slice: AudioBuffer::new(slice_id),
            slice_id,
            tentative_transcript_opt: None,
            user_silent: false,
            last_silent_time: Instant::now(),
            last_tokens: VecDeque::with_capacity(TOKENS_TO_KEEP),
        }
    }

    pub fn clear(&mut self) {
        eprintln!("{}: clearing director", self.slice_id);
        self.last_request = None;
        self.tentative_transcript_opt = None;
        self.user_silent = true;
    }

    pub fn set_silent(&mut self) {
        self.user_silent = true;
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: &DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        // it seems like we get called with a single packet of silence right
        // after we are marked as silent.  Look for this and discard it.
        if self.user_silent && discord_audio.iter().all(|&x| x == 0) {
            // eprintln!("{}: ignoring silent audio", self.slice_id);
            return;
        }

        self.user_silent = false;
        self.user_idle = false;

        if !self.slice.can_fit_audio(rtc_timestamp, discord_audio) {
            eprintln!("{}: slice is full", self.slice_id);
        }
        self.slice.add_audio(rtc_timestamp, discord_audio);

        if self.tentative_transcript_opt.is_some() {
            // eprintln!("discarding tentative transcription");
            self.tentative_transcript_opt = None;
        }
    }

    fn is_ready_for_transcription(&mut self) -> bool {
        if self.slice.is_empty() {
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
            // if the user is silent, then we need to request the full
            // buffer, even if no period shift has occurred
            // in this case we'll have two outstanding transcription
            // requests...
            if self.last_silent_time.elapsed() > SILENT_TIMEOUT {
                self.last_silent_time = Instant::now();
                return true;
            } else {
                // eprintln!("{}: user is silent, but not requesting transcription due to hysteresis prevention", self.slice_id);
            }
        }

        let current_period =
            self.slice.buffer_duration().as_millis() / AUTO_TRANSCRIPTION_PERIOD_MS;
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
        if let Some((_, start_time)) = self.slice.start_time {
            let duration = self.slice.buffer_duration();
            eprintln!(
                "{}: requesting transcription for {} ms",
                self.slice_id,
                duration.as_millis()
            );
            let new_request = LastRequestInfo {
                start_time,
                audio_trimmed_since_request: Duration::ZERO,
                original_duration: self.slice.buffer_duration(),
                in_progress: true,
                requested_at: SystemTime::now(),
                final_request: self.user_idle,
            };
            if let Some(last_request) = self.last_request.as_ref() {
                if last_request.start_time == new_request.start_time
                    && last_request.original_duration == new_request.original_duration
                {
                    // eprintln!("{}: discarding duplicate request", self.slice_id);
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

            return Some(TranscriptionRequest {
                audio_bytes: self.slice.get_bytes(),
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

    pub fn handle_user_idle(&mut self) -> Option<Transcription> {
        self.user_idle = true;

        eprintln!(
            "finalizing slice with {} ms of audio",
            self.slice.buffer_duration().as_millis()
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
            tentative_transcript.audio_duration == self.slice.buffer_duration(),
            "tentative transcription duration != buffer duration",
        );
        self.clear();

        Some(tentative_transcript)
    }

    fn shortcut_tentative_transcripts(&self, message: &Transcription) -> bool {
        // has enough time elapsed since the last request that we can
        // expect audio to have been in the buffer?
        let end_of_transcript = message.start_timestamp + message.audio_duration;
        let time_since_transcript_end =
            SystemTime::now().duration_since(end_of_transcript).unwrap();
        if time_since_transcript_end < USER_SILENCE_TIMEOUT {
            // eprintln!("{} too soon to fast-track", self.slice_id);
            return false;
        }

        // did we receive any audio from the silent period?
        self.slice
            .is_interval_silent(&message.audio_duration, &USER_SILENCE_TIMEOUT)
    }

    pub fn handle_transcription_response(
        &mut self,
        message: &Transcription,
    ) -> Option<Transcription> {
        eprintln!(
            "{}: processing time was {:?}",
            self.slice_id, message.processing_time
        );
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

        let lag = SystemTime::now()
            .duration_since(message.start_timestamp + message.audio_duration)
            .unwrap();
        eprintln!("{}: lag was {:?}", self.slice_id, lag);
        if lag > MAX_LAG {
            for _ in 0..10 {
                eprintln!("{}: the lag is too damn high!!!", self.slice_id);
            }
        }

        // todo: by this point more audio has been received, so we
        // have the data to know if the ends of our tentative segments
        // were followed by silence or not.  If not, we should discard
        // the tentative segments, but if they were, we can finalize
        // them now.
        // for now, just do this on a full-transcript basis, but it should
        // also be possible to do per-segment.
        if self.shortcut_tentative_transcripts(message) {
            // eprintln!("fast-tracking tentative transcripts");
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

        // eprintln!(
        //     "have transcription: {} final segments ({} ms), {} tentative segments ({} ms)",
        //     user_idle_transcript.segments.len(),
        //     user_idle_transcript.audio_duration.as_millis(),
        //     tentative_transcript.segments.len(),
        //     tentative_transcript.audio_duration.as_millis(),
        // );

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

        if self.slice.buffer_duration() == tentative_transcript.audio_duration {
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

    fn discard_audio(&mut self, duration: &Duration) {
        self.slice.discard_audio(duration);
        // also update the last_request duration
        // call trim_audio to update the last_request
        if let Some(last_request) = self.last_request.as_mut() {
            last_request.audio_trimmed_since_request += *duration;
        }
    }
}
