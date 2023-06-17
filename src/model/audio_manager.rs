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
    events::audio::{DiscordAudioData, TranscriptionResponse},
    model::constants::DISCARD_USER_AUDIO_AFTER,
};

use super::{
    super::Whisper,
    types::{self, VoiceChannelEvent},
    user_audio::UserAudio,
};

/// Handles a set of audio buffers, one for each user who is
/// talking in the conversation.
pub(crate) struct AudioManager {
    // these are the buffers which we've assigned to a user
    // in the conversation.  We'll keep them around until some
    // period of time after the user has stopped talking.
    // The tuple stored is:
    //   (time of last activity, buffer)
    user_audio_map: HashMap<types::UserId, (Instant, UserAudio)>,

    // we'll have a single task which monitors this queue for new
    // audio data and then pushes it into the appropriate buffer.
    rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,

    // this queue is used to notify the audio buffer manager that
    // a user has stopped talking.  We'll use this to know when
    // to trigger our final transcription.
    rx_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,

    // this is used to signal the audio buffer manager to shut down.
    shutdown_token: CancellationToken,
}

impl<'a> AudioManager {
    pub fn monitor(
        rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,
        rx_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,
        shutdown_token: CancellationToken,
        tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        whisper: Whisper,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer_manager = AudioManager {
            rx_audio_data,
            rx_silent_user_events,
            shutdown_token,
            user_audio_map: HashMap::new(),
        };
        task::spawn(async move {
            audio_buffer_manager.loop_forever(tx_api, whisper).await;
        })
    }

    /// Helper function, which will lookup or allocate a buffer for
    /// the given user, and then call the given function with a
    /// mutable reference to that buffer.
    fn with_buffer_for_user(
        &mut self,
        user_id: types::UserId,
        mut yield_fn: impl FnMut(&mut UserAudio),
    ) {
        // insert a new buffer if we don't have one for this user
        let (last_activity, buffer) = match self.user_audio_map.entry(user_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert((Instant::now(), UserAudio::new(user_id))),
        };
        (yield_fn)(buffer);
        *last_activity = Instant::now();
    }

    /// Helper function, which will check the given buffer for any
    /// pending transcription requests, and if any are found, will
    /// spawn a task to process them.
    /// Also, will send any completed transcriptions to the API.
    fn maybe_request_transcription(
        whisper: &Arc<Whisper>,
        pending_requests: &mut FuturesUnordered<JoinHandle<TranscriptionResponse>>,
        buffer: &mut UserAudio,
    ) {
        for request in buffer.try_get_transcription_requests() {
            let whisper_clone = whisper.clone();
            let join_handle =
                tokio::spawn(
                    async move { whisper_clone.process_transcription_request(request).await },
                );
            pending_requests.push(join_handle);
        }
    }

    /// Worker function, which will loop forever, processing audio
    /// data and transcription requests.
    async fn loop_forever(
        &mut self,
        tx_api: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        whisper_raw: Whisper,
    ) {
        let whisper = Arc::new(whisper_raw);
        let mut pending_transcription_requests =
            FuturesUnordered::<JoinHandle<TranscriptionResponse>>::new();

        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // we've been asked to shut down
                    return;
                }
                Some(DiscordAudioData {user_id,timestamp, discord_audio,}) = self.rx_audio_data.recv() => {
                    // there's new audio for this user
                    self.with_buffer_for_user(user_id, |buffer| {
                        buffer.add_audio(timestamp, &discord_audio);
                        Self::maybe_request_transcription(
                            &whisper,
                            &mut pending_transcription_requests,
                            buffer,
                        )
                    });
                }
                Some(user_id) = self.rx_silent_user_events.recv() => {
                    // this user has stopped talking for long enough, see if
                    // we have any audio left to finalize
                    self.with_buffer_for_user(user_id, |buffer| {
                        let transcripts = buffer.handle_user_silence();
                        for transcript in transcripts {
                            tx_api
                            .send(VoiceChannelEvent::Transcription(
                                transcript,
                            ))
                            .unwrap();
                        }
                        Self::maybe_request_transcription(
                            &whisper,
                            &mut pending_transcription_requests,
                            buffer,
                        )
                    });
                }
                Ok(Some(transcription_response)) = pending_transcription_requests.try_next() => {
                    // we got a transcription response, determine if it's a final transcription
                    // and if so send it to the API
                    self.with_buffer_for_user(transcription_response.0.user_id, |buffer| {
                        let message_opt = buffer.handle_transcription_response(&transcription_response.0);
                        if let Some(message) = message_opt {
                            tx_api
                            .send(VoiceChannelEvent::Transcription(
                                message,
                            ))
                            .unwrap();
                        }
                        Self::maybe_request_transcription(
                            &whisper,
                            &mut pending_transcription_requests,
                            buffer,
                        )
                    });
                }
            }

            // look through every buffer, and discard any which haven't been
            // updated in the past DISCARD_USER_AUDIO_AFTER period
            let now = Instant::now();
            self.user_audio_map.retain(|_, (last_activity, _)| {
                now.duration_since(*last_activity) < DISCARD_USER_AUDIO_AFTER
            });
        }
    }
}
