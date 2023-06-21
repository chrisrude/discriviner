use std::time::Duration;

use crate::{audio::events::UserAudioEventType, model::types::Transcription};

use super::strategy_trait::{TranscriptStrategy, WorkerActions, WorkerContext};

pub(crate) struct DefaultTranscriptStrategy {}

impl DefaultTranscriptStrategy {
    pub(crate) fn new() -> Self {
        DefaultTranscriptStrategy {}
    }
}

impl TranscriptStrategy for DefaultTranscriptStrategy {
    fn handle_event(&mut self, event: &UserAudioEventType) -> Option<Vec<WorkerActions>> {
        match event {
            UserAudioEventType::Speaking => None,
            UserAudioEventType::Silent => None,
            UserAudioEventType::Idle => {
                //
                // make a new transcription right now
                Some(vec![WorkerActions::NewTranscript(Some(Duration::ZERO))])
            }
        }
    }

    fn handle_transcription(
        &mut self,
        transcript: &Transcription,
        _context: WorkerContext,
    ) -> Option<Vec<WorkerActions>> {
        // just take the whole transcription
        Some(vec![WorkerActions::Publish(transcript.clone())])
    }
}
