use std::time::Duration;

use crate::{
    audio::events::UserAudioEventType,
    model::{
        constants::{AUTO_TRANSCRIPTION_PERIOD, USER_SILENCE_TIMEOUT},
        types::Transcription,
    },
};

use super::strategy_trait::{TranscriptStrategy, WorkerActions, WorkerContext};

pub(crate) struct FiveSecondStrategy {
    tentative_transcript_opt: Option<Transcription>,
}

impl FiveSecondStrategy {
    pub(crate) fn new() -> Self {
        FiveSecondStrategy {
            tentative_transcript_opt: None,
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
                AUTO_TRANSCRIPTION_PERIOD,
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

        if context.silent_after {
            // if the time after the end of the transcript is silent,
            // then we can return the transcript as-is.
            return Some(vec![WorkerActions::Publish(transcript.clone())]);
        }

        let end_time =
            transcript.start_timestamp + transcript.audio_duration - USER_SILENCE_TIMEOUT;

        let (finalized_transcript, tentative_transcript) =
            Transcription::split_at_end_time(&transcript, end_time);

        self.tentative_transcript_opt = if context.audio_duration
            == tentative_transcript.audio_duration
            && !tentative_transcript.is_empty()
        {
            eprintln!(
                "Saving tentative transcript: {}",
                tentative_transcript.text()
            );
            Some(tentative_transcript)
        } else {
            None
        };

        Some(vec![
            WorkerActions::Publish(finalized_transcript),
            WorkerActions::NewTranscript(Some(AUTO_TRANSCRIPTION_PERIOD)),
        ])
    }
}
