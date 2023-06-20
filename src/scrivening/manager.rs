use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    time::Instant,
};

use futures::{stream::FuturesUnordered, TryStreamExt};
use tokio::{
    sync,
    task::{self, JoinHandle},
};
use tokio_util::sync::CancellationToken;

use crate::{
    audio::events::{DiscordAudioData, TranscriptionResponse, UserAudioEvent, UserAudioEventType},
    model::{
        constants::DISCARD_USER_AUDIO_AFTER,
        types::{Transcription, UserId, VoiceChannelEvent},
    },
    scrivening::transcript_director::TranscriptDirector,
};

use super::super::Whisper;

/// Creates an audio buffer for each user who is talking in the conversation.
/// Takes in events related to those users, and forwards them to the
/// appropriate buffer.
pub(crate) struct UserAudioEventDemux {
    // these are the buffers which we've assigned to a user
    // in the conversation.  We'll keep them around until some
    // period of time after the user has stopped talking.
    // The tuple stored is:
    //   (time of last activity, buffer)
    user_audio_map: HashMap<UserId, (sync::mpsc::UnboundedSender<UserAudioEventType>, Instant)>,

    tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,

    // this is used to signal the audio buffer manager to shut down.
    shutdown_token: CancellationToken,

    whisper: Arc<Whisper>,
}

impl UserAudioEventDemux {
    pub fn monitor(
        rx_audio_data: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
        rx_silent_user_events: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
        shutdown_token: CancellationToken,
        tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        whisper: Whisper,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer_manager = UserAudioEventDemux {
            shutdown_token,
            tx_api,
            user_audio_map: HashMap::new(),
            whisper: Arc::new(whisper),
        };
        task::spawn(async move {
            audio_buffer_manager
                .loop_forever(rx_audio_data, rx_silent_user_events)
                .await;
        })
    }

    /// Helper function, which will lookup or allocate a buffer for
    /// the given user, and then call the given function with a
    /// mutable reference to that buffer.
    fn send_to_worker(&mut self, event: UserAudioEvent) {
        // insert a new buffer if we don't have one for this user
        let (tx_worker, last_activity) = match self.user_audio_map.entry(event.user_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let tx_worker = UserAudioWorker::new(
                    self.shutdown_token.clone(),
                    self.tx_api.clone(),
                    event.user_id,
                    self.whisper.clone(),
                );
                entry.insert((tx_worker, Instant::now()))
            }
        };
        tx_worker.send(event.event_type).unwrap();
        *last_activity = Instant::now();
    }

    /// Worker function, which will loop forever, processing audio
    /// data and transcription requests.
    async fn loop_forever(
        &mut self,
        mut rx_audio_data: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
        mut rx_silent_user_events: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
    ) {
        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // we've been asked to shut down
                    return;
                }
                Some( user_audio_event ) = rx_audio_data.recv() => {
                    self.send_to_worker(user_audio_event);
                }
                Some( user_audio_event ) = rx_silent_user_events.recv() => {
                    // there's new audio for this user
                    self.send_to_worker(user_audio_event);
                }
            }

            // look through every buffer, and discard any which haven't been
            // updated in the past DISCARD_USER_AUDIO_AFTER period
            let now = Instant::now();
            self.user_audio_map.retain(|_, (_, last_activity)| {
                now.duration_since(*last_activity) < DISCARD_USER_AUDIO_AFTER
            });
        }
    }
}

struct UserAudioWorker {
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
    fn new(
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
