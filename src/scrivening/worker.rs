use std::{collections::VecDeque, sync::Arc};

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
        types::{Transcription, UserId, VoiceChannelEvent},
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
                        pending_transcription_requests.push(
                            self.whisper
                            .process_transcription_request(transcription_request)
                        );
                    }
                    next_transcription_time.as_mut().reset(never);
                    None
                }
                Some(DiscordAudioData{discord_audio,rtc_timestamp,..}) = rx_audio.recv() => {
                    self.audio_buffer.add_audio(&rtc_timestamp, discord_audio.as_slice());
                    None
                }
                Some(event) = rx_event.recv() => {
                    transcript_strategy.handle_event(&event)
                }
                Ok(Some(TranscriptionResponse{ transcript })) = pending_transcription_requests.try_next() => {
                    // we got a transcription response, determine if it's a final transcription
                    // and if so send it to the API
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
        transcription: Transcription,
        tx_api: &UnboundedSender<VoiceChannelEvent>,
    ) {
        // remove the audio associated with this transcription
        self.audio_buffer
            .discard_audio(&transcription.audio_duration);

        // if the transcription is empty, don't send it.
        // we still needed to remove the audio, though.
        if transcription.segments.is_empty() {
            return;
        }

        // add the tokens from this transcription to our last_tokens
        self.last_tokens.add_all(&transcription.token_ids());

        // send the transcription to the API
        tx_api
            .send(VoiceChannelEvent::Transcription(transcription))
            .unwrap();
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
