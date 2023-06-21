use std::time::Duration;

use crate::{audio::events::UserAudioEventType, model::types::Transcription};

/// Determines when to start a new transcript generation,
/// and when to consider a segment from a transcript complete.

pub(crate) enum WorkerActions {
    /// Requests that a new transcript be generated
    /// at the given point in the future.  This will replace
    /// any previous pending request for a transcript.
    /// If None, no new transcript will be generated.
    NewTranscript(Option<Duration>),

    /// Publish the given transcript to the API.
    Publish(Transcription),
}

pub(crate) struct WorkerContext {
    pub audio_duration: Duration,
    pub silent_after: bool,
}

pub(crate) trait TranscriptStrategy {
    /// Called when a user audio event happens, e.g.:
    ///     Speaking - user starts speaking
    ///     Silent - user stops speaking for 100ms
    ///     Idle - user stops speaking for 1 second
    fn handle_event(&mut self, event: &UserAudioEventType) -> Option<Vec<WorkerActions>>;

    fn handle_transcription(
        &mut self,
        transcript: &Transcription,
        context: WorkerContext,
    ) -> Option<Vec<WorkerActions>>;
}

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
