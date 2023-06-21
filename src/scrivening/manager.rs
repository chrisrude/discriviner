use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    time::Instant,
};

use tokio::{
    sync::{self, mpsc::UnboundedSender},
    task,
};
use tokio_util::sync::CancellationToken;

use crate::{
    audio::events::{DiscordAudioData, UserAudioEvent, UserAudioEventType},
    model::{
        constants::DISCARD_USER_AUDIO_AFTER,
        types::{UserId, VoiceChannelEvent},
    },
    strategies::five_second_strategy::FiveSecondStrategy,
};

use super::{super::Whisper, worker::UserAudioWorker};

/// Creates an audio buffer for each user who is talking in the conversation.
/// Takes in events related to those users, and forwards them to the
/// appropriate buffer.
pub(crate) struct UserAudioManager {
    // these are the buffers which we've assigned to a user
    // in the conversation.  We'll keep them around until some
    // period of time after the user has stopped talking.
    // The tuple stored is:
    //   (time of last activity, buffer)
    user_audio_map: HashMap<
        UserId,
        (
            UnboundedSender<UserAudioEventType>,
            UnboundedSender<DiscordAudioData>,
            Instant,
        ),
    >,

    tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,

    // this is used to signal the audio buffer manager to shut down.
    shutdown_token: CancellationToken,

    whisper: Arc<Whisper>,
}

impl UserAudioManager {
    pub fn monitor(
        rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,
        rx_silent_user_events: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
        shutdown_token: CancellationToken,
        tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        whisper: Whisper,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer_manager = UserAudioManager {
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

    fn get_worker(
        &mut self,
        user_id: UserId,
    ) -> &mut (
        UnboundedSender<UserAudioEventType>,
        UnboundedSender<DiscordAudioData>,
        Instant,
    ) {
        // insert a new buffer if we don't have one for this user
        match self.user_audio_map.entry(user_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let (tx_worker, tx_audio) = UserAudioWorker::monitor(
                    self.shutdown_token.clone(),
                    FiveSecondStrategy::new(),
                    self.tx_api.clone(),
                    user_id,
                    self.whisper.clone(),
                );
                entry.insert((tx_worker, tx_audio, Instant::now()))
            }
        }
    }

    /// Helper function, which will lookup or allocate a buffer for
    /// the given user, and then call the given function with a
    /// mutable reference to that buffer.
    fn send_to_worker(&mut self, event: UserAudioEvent) {
        let (tx_worker, _, last_activity) = self.get_worker(event.user_id);
        tx_worker.send(event.event_type).unwrap();
        *last_activity = Instant::now();
    }

    fn send_audio_to_worker(&mut self, audio: DiscordAudioData) {
        let (_, tx_audio, last_activity) = self.get_worker(audio.user_id);
        tx_audio.send(audio).unwrap();
        *last_activity = Instant::now();
    }

    /// Worker function, which will loop forever, processing audio
    /// data and transcription requests.
    async fn loop_forever(
        &mut self,
        mut rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,
        mut rx_silent_user_events: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
    ) {
        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // we've been asked to shut down
                    return;
                }
                Some( user_audio_event ) = rx_audio_data.recv() => {
                    self.send_audio_to_worker(user_audio_event);
                }
                Some( user_audio_event ) = rx_silent_user_events.recv() => {
                    // there's new audio for this user
                    self.send_to_worker(user_audio_event);
                }
            }

            // look through every buffer, and discard any which haven't been
            // updated in the past DISCARD_USER_AUDIO_AFTER period
            let now = Instant::now();
            self.user_audio_map.retain(|_, (_, _, last_activity)| {
                now.duration_since(*last_activity) < DISCARD_USER_AUDIO_AFTER
            });
        }
    }
}
