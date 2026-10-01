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

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use crate::model::types::{TextSegment, TokenWithProbability};

    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn segment(start_offset_ms: u32, end_offset_ms: u32) -> TextSegment {
        TextSegment {
            tokens_with_probability: vec![TokenWithProbability {
                token_id: 0,
                token_text: "word".to_string(),
                p: 90,
            }],
            start_offset_ms,
            end_offset_ms,
        }
    }

    fn transcription(segments: Vec<TextSegment>, audio_duration: Duration) -> Transcription {
        Transcription {
            segments,
            start_timestamp: SystemTime::UNIX_EPOCH,
            user_id: 1,
            audio_duration,
            processing_time: ms(100),
        }
    }

    fn context(audio_duration: Duration, silent_after: bool) -> WorkerContext {
        WorkerContext {
            audio_duration,
            silent_after,
        }
    }

    #[test]
    fn test_next_transcript_time_before_first_period() {
        let strategy = FiveSecondStrategy::new();
        assert_eq!(strategy.get_next_transcript_time(&ms(0)), ms(5000));
        assert_eq!(strategy.get_next_transcript_time(&ms(3000)), ms(2000));
        assert_eq!(strategy.get_next_transcript_time(&ms(4999)), ms(1));
    }

    #[test]
    fn test_next_transcript_time_after_first_period() {
        let strategy = FiveSecondStrategy::new();
        // on a period boundary, wait a full period
        assert_eq!(strategy.get_next_transcript_time(&ms(5000)), ms(1000));
        assert_eq!(strategy.get_next_transcript_time(&ms(7000)), ms(1000));
        // otherwise, wait until the next boundary
        assert_eq!(strategy.get_next_transcript_time(&ms(5300)), ms(700));
        assert_eq!(strategy.get_next_transcript_time(&ms(12_999)), ms(1));
    }

    #[test]
    fn test_speaking_schedules_next_transcript() {
        let mut strategy = FiveSecondStrategy::new();
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Speaking, &ms(2000)),
            Some(vec![WorkerActions::NewTranscript(Some(ms(3000)))])
        );
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Speaking, &ms(6400)),
            Some(vec![WorkerActions::NewTranscript(Some(ms(600)))])
        );
    }

    #[test]
    fn test_silent_requests_transcript_immediately() {
        let mut strategy = FiveSecondStrategy::new();
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Silent, &ms(2000)),
            Some(vec![WorkerActions::NewTranscript(Some(Duration::ZERO))])
        );
    }

    #[test]
    fn test_idle_without_tentative_transcript_does_nothing() {
        let mut strategy = FiveSecondStrategy::new();
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(2000)),
            None
        );
    }

    #[test]
    fn test_transcription_followed_by_silence_is_published_whole() {
        let mut strategy = FiveSecondStrategy::new();
        let transcript = transcription(vec![segment(0, 1000), segment(1000, 2900)], ms(3000));
        assert_eq!(
            strategy.handle_transcription(&transcript, context(ms(3000), true)),
            Some(vec![
                WorkerActions::Publish(transcript.clone()),
                WorkerActions::NewTranscript(Some(FIRST_TRANSCRIPT_PERIOD)),
            ])
        );
    }

    #[test]
    fn test_transcription_published_whole_when_buffer_two_thirds_full() {
        let mut strategy = FiveSecondStrategy::new();
        let transcript = transcription(vec![segment(0, 19_900)], ms(20_000));
        let buffer_duration = 2 * AUDIO_TO_RECORD / 3;
        assert_eq!(
            strategy.handle_transcription(&transcript, context(buffer_duration, false)),
            Some(vec![
                WorkerActions::Publish(transcript.clone()),
                WorkerActions::NewTranscript(Some(FIRST_TRANSCRIPT_PERIOD)),
            ])
        );
    }

    #[test]
    fn test_transcription_while_speaking_finalizes_all_but_the_tail() {
        let mut strategy = FiveSecondStrategy::new();
        // the last USER_SILENCE_TIMEOUT of audio may be cut off mid-word,
        // so segments that end in it are held back
        let transcript = transcription(vec![segment(0, 1000), segment(1000, 2900)], ms(3000));
        let actions = strategy
            .handle_transcription(&transcript, context(ms(3000), false))
            .unwrap();

        let mut finalized = transcription(vec![segment(0, 1000)], ms(1000));
        finalized.processing_time = transcript.processing_time;
        assert_eq!(
            actions,
            vec![
                WorkerActions::Publish(finalized),
                // 2000 ms remain, so the next transcript is due at the 5 s mark
                WorkerActions::NewTranscript(Some(ms(3000))),
            ]
        );
    }

    #[test]
    fn test_tentative_transcript_published_on_idle_if_no_new_audio() {
        let mut strategy = FiveSecondStrategy::new();
        // nothing ends before the tail, so nothing is finalized and the
        // whole transcript is held as tentative
        let transcript = transcription(vec![segment(0, 2900)], ms(3000));
        let actions = strategy
            .handle_transcription(&transcript, context(ms(3000), false))
            .unwrap();
        assert!(matches!(&actions[0], WorkerActions::Publish(t) if t.is_empty()));

        // the user goes idle and the buffer hasn't grown: publish as-is
        match strategy
            .handle_event(&UserAudioEventType::Idle, &ms(3000))
            .as_deref()
        {
            Some([WorkerActions::Publish(t)]) => {
                assert_eq!(t.segments.len(), 1);
                assert_eq!(t.audio_duration, ms(3000));
            }
            other => panic!("expected the tentative transcript, got {:?}", other),
        }

        // it is only published once
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(3000)),
            None
        );
    }

    #[test]
    fn test_tentative_transcript_discarded_if_audio_arrived() {
        let mut strategy = FiveSecondStrategy::new();
        let transcript = transcription(vec![segment(0, 2900)], ms(3000));
        strategy.handle_transcription(&transcript, context(ms(3000), false));

        // more audio came in after the transcript was requested, so the
        // tentative transcript is stale
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(3500)),
            None
        );
        // and it doesn't come back once the durations happen to match
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(3000)),
            None
        );
    }

    #[test]
    fn test_new_transcription_replaces_tentative_transcript() {
        let mut strategy = FiveSecondStrategy::new();
        let transcript = transcription(vec![segment(0, 2900)], ms(3000));
        strategy.handle_transcription(&transcript, context(ms(3000), false));
        strategy.handle_transcription(&transcript, context(ms(3000), true));
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(3000)),
            None
        );
    }

    #[test]
    fn test_no_tentative_transcript_when_part_was_finalized() {
        // Current behavior: a tentative transcript is kept only when its
        // duration equals the whole buffer, i.e. when nothing was finalized.
        // After a partial finalize, the tail is never published from Idle
        // and has to wait for the next whisper run.
        let mut strategy = FiveSecondStrategy::new();
        let transcript = transcription(vec![segment(0, 1000), segment(1000, 2900)], ms(3000));
        strategy.handle_transcription(&transcript, context(ms(3000), false));
        // after publishing the finalized 1000 ms, the buffer holds 2000 ms
        assert_eq!(
            strategy.handle_event(&UserAudioEventType::Idle, &ms(2000)),
            None
        );
    }
}
