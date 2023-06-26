use std::{collections::VecDeque, sync::Arc, time::Duration};

use futures::{stream::FuturesUnordered, TryStreamExt};
use tokio::{
    sync::{
        self,
        mpsc::{UnboundedReceiver, UnboundedSender},
    },
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use whisper_rs::WhisperToken;

use crate::{
    audio::{
        audio_buffer::AudioBuffer,
        events::{DiscordAudioData, TranscriptionResponse, UserAudioEventType},
    },
    model::{
        constants::{TOKENS_TO_KEEP, USER_SILENCE_TIMEOUT},
        types::{TextSegment, TokenWithProbability, Transcription, UserId, VoiceChannelEvent},
    },
    strategies::strategy_trait::{TranscriptStrategy, WorkerActions, WorkerContext},
};

use super::super::Whisper;

pub(crate) struct UserAudioWorker {
    audio_buffer: AudioBuffer,

    last_tokens: BoundedTokenBuffer,

    shutdown_token: CancellationToken,

    whisper: Arc<Whisper>,
}

impl Drop for UserAudioWorker {
    fn drop(&mut self) {
        // make our worker task exit
        self.shutdown_token.cancel();
    }
}

/// If the strategy doesn't set a transcription time, we'll use this as a failsafe.
/// This is expressed as a percentage of the audio buffer duration.
const FAILSAFE_TRANSCRIPTION_PERCENTAGE: f32 = 0.8;

/// if we have this many tokens in a single segment, we'll assume the
/// AI is hallucinating and ignore it
const OUTRAGEOUSLY_MANY_TOKENS: usize = 100;

impl UserAudioWorker {
    pub(crate) fn monitor<T>(
        shutdown_token: CancellationToken,
        transcript_strategy: T,
        tx_api: UnboundedSender<VoiceChannelEvent>,
        user_id: UserId,
        whisper: Arc<Whisper>,
    ) -> (
        UnboundedSender<UserAudioEventType>,
        UnboundedSender<DiscordAudioData>,
    )
    where
        T: TranscriptStrategy + Send + Sync + 'static,
    {
        let (tx_event, rx_event) = sync::mpsc::unbounded_channel::<UserAudioEventType>();
        let (tx_audio, rx_audio) = sync::mpsc::unbounded_channel::<DiscordAudioData>();

        // start our worker thread
        tokio::spawn(
            Self {
                audio_buffer: AudioBuffer::new(user_id),
                last_tokens: BoundedTokenBuffer::new(),
                shutdown_token,
                whisper,
            }
            .loop_forever(rx_event, rx_audio, transcript_strategy, tx_api),
        );
        (tx_event, tx_audio)
    }

    async fn loop_forever<T>(
        mut self,
        mut rx_event: UnboundedReceiver<UserAudioEventType>,
        mut rx_audio: UnboundedReceiver<DiscordAudioData>,
        mut transcript_strategy: T,
        tx_api: UnboundedSender<VoiceChannelEvent>,
    ) where
        T: TranscriptStrategy + Send + Sync,
    {
        let mut pending_transcription_requests =
            FuturesUnordered::<JoinHandle<TranscriptionResponse>>::new();

        let never = Instant::now() + time::Duration::from_secs(1000 * 1000 * 1000);

        // start without a transcription time, let the transcript strategy decide when to start
        let next_transcription_time = time::sleep_until(never);
        tokio::pin!(next_transcription_time);

        loop {
            if let Some(actions) = tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // we've been asked to shut down
                    break;
                }
                _ = &mut next_transcription_time => {
                    // request transcription
                    if let Some(transcription_request) = self.audio_buffer.make_transcription_request(
                        self.last_tokens.get(),
                    ) {
                        if pending_transcription_requests.is_empty() {
                            pending_transcription_requests.push(
                                self.whisper
                                .process_transcription_request(transcription_request)
                            );
                        }
                    }
                    next_transcription_time.as_mut().reset(never);
                    None
                }
                Some(DiscordAudioData{discord_audio,rtc_timestamp,..}) = rx_audio.recv() => {
                    self.audio_buffer.add_audio(&rtc_timestamp, discord_audio.as_slice());
                    None
                }
                Some(event) = rx_event.recv() => {
                    transcript_strategy.handle_event(&event, &self.audio_buffer.buffer_duration())
                }
                Ok(Some(TranscriptionResponse{ transcript })) = pending_transcription_requests.try_next() => {
                    // we got a transcription response, determine if it's a final transcription
                    // and if so send it to the API
                    eprintln!(
                        "received transcription ({:?} ms): {}",
                        transcript.audio_duration,
                        transcript.text()
                    );
                    self.print_rms(&transcript);

                    transcript_strategy.handle_transcription(&transcript, WorkerContext {
                        audio_duration: self.audio_buffer.buffer_duration(),
                        silent_after: self.audio_buffer.is_interval_silent(
                            &transcript.audio_duration,
                            &USER_SILENCE_TIMEOUT,
                        )
                    })
                }
            } {
                for action in actions {
                    match action {
                        WorkerActions::NewTranscript(duration_opt) => {
                            next_transcription_time.as_mut().reset(
                                duration_opt
                                    .map_or(never, |duration| time::Instant::now() + duration),
                            );
                        }
                        WorkerActions::Publish(transcription) => {
                            self.publish(transcription, &tx_api);
                        }
                    }
                }
            }
            // sanity check on the pending transcription requests
            if !self.audio_buffer.is_empty() && pending_transcription_requests.is_empty() {
                let next_transcription_delay = next_transcription_time
                    .deadline()
                    .duration_since(Instant::now());
                if self.audio_buffer.remaining_capacity() < next_transcription_delay {
                    next_transcription_time.as_mut().reset(
                        time::Instant::now()
                            + time::Duration::from_secs_f32(
                                FAILSAFE_TRANSCRIPTION_PERCENTAGE
                                    * self.audio_buffer.buffer_duration().as_secs_f32(),
                            ),
                    );
                }
            }
        }
        // exit!
    }

    /// Publish a transcription to the API
    /// This is called when we have a final transcription.
    /// In addition, publishing has these side-effects:
    /// - the audio associated with the transcription is removed from the buffer
    /// - the tokens associated with the transcription are added to last_tokens
    fn publish(
        &mut self,
        mut transcription: Transcription,
        tx_api: &UnboundedSender<VoiceChannelEvent>,
    ) {
        // remove the audio associated with this transcription
        self.audio_buffer
            .discard_audio(&transcription.audio_duration);

        // filter out any "spurious" segments from the transcription
        transcription.segments.retain(is_valid_segment);

        // if the transcription is empty, don't send it.
        // we still needed to remove the audio, though.
        if transcription.segments.is_empty() {
            return;
        }

        // add the tokens from this transcription to our last_tokens
        self.last_tokens.add_all(&transcription.token_ids());

        // send the transcription to the API
        match tx_api.send(VoiceChannelEvent::Transcription(transcription)) {
            Ok(_) => {} // everything is fine
            Err(err) => {
                eprintln!("error sending transcription to API: {}", err);
            }
        }
    }

    fn print_rms(&self, transcription: &Transcription) {
        // before we drop the audio data, calculate the RMS energy
        // of the audio data and add it to the transcription
        if let Some((_, start_time)) = self.audio_buffer.start_time {
            if transcription.start_timestamp != start_time {
                eprintln!(
                    "transcription start time ({:?}) does not match audio buffer start time ({:?})",
                    transcription.start_timestamp, start_time
                );
            } else {
                // take the RMS over the whole duration
                let audio_rms = self
                    .audio_buffer
                    .rms_over_interval(&Duration::ZERO, &transcription.audio_duration);
                eprintln!(
                    "transcription ({:?} ms) rms: {}",
                    transcription.audio_duration.as_millis(),
                    audio_rms
                );

                // also, do it for all the segments.
                // Obviously we can do this more efficiently, but this is just for debugging.
                for (i, segment) in transcription.segments.iter().enumerate() {
                    let segment_start = Duration::from_millis(segment.start_offset_ms as u64);
                    let segment_length = Duration::from_millis(
                        (segment.end_offset_ms - segment.start_offset_ms) as u64,
                    );
                    let audio_rms = self
                        .audio_buffer
                        .rms_over_interval(&segment_start, &segment_length);
                    eprintln!(
                        "segment {} ({:?} ms) rms: {}",
                        i,
                        segment_length.as_millis(),
                        audio_rms
                    );
                }
            }
        } else {
            eprintln!("audio buffer has no start time");
        }
    }
}

struct BoundedTokenBuffer(VecDeque<WhisperToken>);

impl BoundedTokenBuffer {
    fn new() -> Self {
        Self(VecDeque::with_capacity(TOKENS_TO_KEEP))
    }

    fn add(&mut self, token: &WhisperToken) {
        if self.0.len() == TOKENS_TO_KEEP {
            self.0.pop_front();
        }
        self.0.push_back(*token);
    }

    fn add_all(&mut self, tokens: &[WhisperToken]) {
        for token in tokens {
            self.add(token);
        }
    }

    fn get(&self) -> Vec<WhisperToken> {
        self.0.iter().cloned().collect()
    }
}

fn probability_histogram(segment: &TextSegment) -> String {
    let mut histogram = [0; 10];
    for TokenWithProbability { p, .. } in segment.tokens_with_probability.iter() {
        assert!(p <= &100);
        let bucket = (p / 10) as usize;
        if 10 == bucket {
            // we don't have a bucket for 100%, so we'll just put it in the 90% bucket
            histogram[9] += 1;
        } else {
            histogram[bucket] += 1;
        }
    }

    const BARS: [&str; 5] = ["▯", "░", "▒", "▓", "█"];

    // normalize the histogram to values 0-4
    // sum all the tokens
    let total = histogram.iter().sum::<usize>();
    for count in histogram.iter_mut() {
        if count > &mut 0 {
            *count = (*count * (BARS.len() - 2)) / total;
            *count += 1;
        } else {
            *count = 0;
        }
    }

    let mut result = String::new();
    result.push('[');
    for count in histogram.iter() {
        result.push_str(BARS[*count]);
    }
    result.push_str("] (");
    result.push_str(&total.to_string());
    result.push_str(" tokens)");
    result
}

/// Whisper is great at spoken words, but isn't great at detecting
/// when it gets audio without any spoken words.  This heuristic
/// is to notice when it's produced a transcript which is probability
/// not at all audio, then discard it.
fn is_valid_segment(segment: &TextSegment) -> bool {
    eprintln!("probability: {}", probability_histogram(segment));

    // discard the transcript if more than half the tokens in it have a
    // probability of 50 or less
    let mut low_probability_tokens = 0;
    let mut high_probability_tokens = 0;
    for TokenWithProbability { p, .. } in segment.tokens_with_probability.iter() {
        if *p <= 50 {
            low_probability_tokens += 1;
        } else {
            high_probability_tokens += 1;
        }
    }
    if low_probability_tokens > high_probability_tokens {
        eprintln!("discarding transcript segment with {} low probability tokens and {} high probability tokens", low_probability_tokens, high_probability_tokens);
        return false;
    }
    if (low_probability_tokens + high_probability_tokens) >= OUTRAGEOUSLY_MANY_TOKENS {
        eprintln!(
            "discarding transcript segment with {} tokens: too many to be real!",
            low_probability_tokens + high_probability_tokens
        );
        return false;
    }
    return true;
}
