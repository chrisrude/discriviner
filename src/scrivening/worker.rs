use std::sync::Arc;

use futures::{stream::FuturesUnordered, TryStreamExt};
use tokio::{sync, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    audio::events::{DiscordAudioData, TranscriptionResponse, UserAudioEventType},
    model::types::{Transcription, UserId, VoiceChannelEvent},
    scrivening::transcript_director::TranscriptDirector,
};

use super::super::Whisper;

pub(crate) struct UserAudioWorker {
    director: TranscriptDirector,
    rx_user_audio_event: sync::mpsc::UnboundedReceiver<UserAudioEventType>,
    tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
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
    pub(crate) fn monitor(
        shutdown_token: CancellationToken,
        tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        user_id: UserId,
        whisper: Arc<Whisper>,
    ) -> sync::mpsc::UnboundedSender<UserAudioEventType> {
        let (tx, rx) = sync::mpsc::unbounded_channel::<UserAudioEventType>();

        // start our worker thread
        tokio::spawn(
            Self {
                director: TranscriptDirector::new(user_id),
                rx_user_audio_event: rx,
                shutdown_token,
                tx_api,
                whisper,
            }
            .loop_forever(),
        );
        tx
    }

    async fn loop_forever(mut self) {
        let mut pending_transcription_requests =
            FuturesUnordered::<JoinHandle<TranscriptionResponse>>::new();

        loop {
            if let Some(request) = self.director.make_transcription_request() {
                pending_transcription_requests
                    .push(self.whisper.process_transcription_request(request));
            }

            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // we've been asked to shut down
                    return;
                }
                Some(event_type) = self.rx_user_audio_event.recv() => {
                    match event_type {
                        UserAudioEventType::Audio(DiscordAudioData{
                            timestamp,
                            discord_audio,
                        }) => {
                            self.director.add_audio(&timestamp, &discord_audio);
                        }
                        UserAudioEventType::Silent => {
                            self.director.set_silent();

                        }
                        UserAudioEventType::Speaking => {
                            self.director.set_speaking();
                        }
                        UserAudioEventType::Idle => {
                            let transcript_opt = self.director.handle_user_idle();
                            self.maybe_send_transcript(transcript_opt);
                        }
                    }
                }
                Ok(Some(TranscriptionResponse{ transcript })) = pending_transcription_requests.try_next() => {
                    // we got a transcription response, determine if it's a final transcription
                    // and if so send it to the API
                    let transcript_opt = self.director.handle_transcription_response(&transcript);
                    self.maybe_send_transcript(transcript_opt);
                }
            }
        }
    }

    fn maybe_send_transcript(&self, transcript_opt: Option<Transcription>) {
        if let Some(transcript) = transcript_opt {
            self.tx_api
                .send(VoiceChannelEvent::Transcription(transcript))
                .unwrap();
        }
    }
}
