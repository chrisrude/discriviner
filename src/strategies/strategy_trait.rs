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
    fn handle_event(
        &mut self,
        event: &UserAudioEventType,
        audio_duration: &Duration,
    ) -> Option<Vec<WorkerActions>>;

    fn handle_transcription(
        &mut self,
        transcript: &Transcription,
        context: WorkerContext,
    ) -> Option<Vec<WorkerActions>>;
}
