use std::time::Duration;

use crate::{
    audio::events::UserAudioEventType,
    model::{
        constants::{AUDIO_TO_RECORD, USER_SILENCE_TIMEOUT},
        types::Transcription,
    },
};

use super::strategy_trait::{TranscriptStrategy, WorkerActions, WorkerContext};

const FIRST_TRANSCRIPT_PERIOD: Duration = Duration::from_secs(5);
const SUBSEQUENT_TRANSCRIPT_PERIOD: Duration = Duration::from_secs(1);

pub(crate) struct FiveSecondStrategy {
    tentative_transcript_opt: Option<Transcription>,
    tentative_transcripts_used: usize,
    tentative_transcripts_total: usize,
}

impl FiveSecondStrategy {
    pub(crate) fn new() -> Self {
        FiveSecondStrategy {
            tentative_transcript_opt: None,
            tentative_transcripts_used: 0,
            tentative_transcripts_total: 0,
        }
    }

    /// Returns the interval between now and when we want to take
    /// the next transcription.  This uses the following logic:
    ///  - if the current audio duration is less than 5 seconds,
    ///    then we want to take a transcription at the 5 second mark.
    ///  - if it's longer, than we want to take the next transcription
    ///    at intervals of 1 second after the last transcription.
    fn get_next_transcript_time(&self, audio_duration: &Duration) -> Duration {
        if audio_duration < &FIRST_TRANSCRIPT_PERIOD {
            FIRST_TRANSCRIPT_PERIOD - *audio_duration
        } else {
            // apparently mod isn't implemented for Duration, so we have to
            // do this the hard way.
            let additional_audio = *audio_duration - FIRST_TRANSCRIPT_PERIOD;
            let remainder_ms =
                (additional_audio.as_millis() % SUBSEQUENT_TRANSCRIPT_PERIOD.as_millis()) as u64;
            SUBSEQUENT_TRANSCRIPT_PERIOD - Duration::from_millis(remainder_ms)
        }
    }
}

impl TranscriptStrategy for FiveSecondStrategy {
    fn handle_event(
        &mut self,
        event: &UserAudioEventType,
        audio_duration: &Duration,
    ) -> Option<Vec<WorkerActions>> {
        match event {
            UserAudioEventType::Speaking => Some(vec![WorkerActions::NewTranscript(Some(
                self.get_next_transcript_time(audio_duration),
            ))]),
            UserAudioEventType::Silent => {
                // request a transcription, now!
                Some(vec![WorkerActions::NewTranscript(Some(Duration::ZERO))])
            }
            UserAudioEventType::Idle => {
                // if we had a tentative transcript, and we haven't gotten
                // more audio since then, then we can return the tentative
                // transcript as-is.
                if let Some(tentative_transcript) = self.tentative_transcript_opt.take() {
                    if audio_duration == &tentative_transcript.audio_duration {
                        self.tentative_transcripts_used += 1;
                        return Some(vec![WorkerActions::Publish(tentative_transcript)]);
                    }
                }
                // todo: decide whether to kick off at Silent or Idle based
                // on past performance
                None
            }
        }
    }

    fn handle_transcription(
        &mut self,
        transcript: &Transcription,
        context: WorkerContext,
    ) -> Option<Vec<WorkerActions>> {
        // clear any previous tentative transcript
        self.tentative_transcript_opt = None;

        // if the time after the end of the transcript is silent,
        // then we can return the transcript as-is.
        //
        // alternatively,
        // if we've filled our buffer up at least 2/3 of the
        // way, just take what we have.  It's possible that
        // whisper is only ever going to give us a single segment.
        let running_out_of_space = context.audio_duration >= (2 * AUDIO_TO_RECORD / 3);
        if context.silent_after || running_out_of_space {
            return Some(vec![
                WorkerActions::Publish(transcript.clone()),
                WorkerActions::NewTranscript(Some(FIRST_TRANSCRIPT_PERIOD)),
            ]);
        }

        let end_time =
            transcript.start_timestamp + transcript.audio_duration - USER_SILENCE_TIMEOUT;

        let (finalized_transcript, tentative_transcript) =
            Transcription::split_at_end_time(transcript, end_time);

        self.tentative_transcript_opt = if context.audio_duration
            == tentative_transcript.audio_duration
            && !tentative_transcript.is_empty()
        {
            self.tentative_transcripts_total += 1;
            if 0 == self.tentative_transcripts_total % 10 {
                eprintln!(
                    "{} tentative transcripts, {} used",
                    self.tentative_transcripts_total, self.tentative_transcripts_used
                );
            }
            Some(tentative_transcript)
        } else {
            None
        };

        let duration_after_finalizing =
            transcript.audio_duration - finalized_transcript.audio_duration;

        Some(vec![
            WorkerActions::Publish(finalized_transcript),
            WorkerActions::NewTranscript(Some(
                self.get_next_transcript_time(&duration_after_finalizing),
            )),
        ])
    }
}
