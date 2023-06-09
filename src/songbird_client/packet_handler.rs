use std::sync::Arc;

/// Manages multiple buffers for each user who is speaking.
/// Tracks when users have stopped speaking, and fires a callback.
use async_trait::async_trait;

use songbird::events::context_data::SpeakingUpdateData;
use songbird::events::context_data::VoiceData;
use songbird::model::payload::Speaking;
use songbird::EventContext;

use std::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;

use crate::audio::events::DiscordAudioData;
use crate::audio::events::UserAudioEvent;
use crate::audio::events::UserAudioEventType;
use crate::model::types;
use crate::model::types::ConnectData;
use crate::model::types::DisconnectData;
use crate::model::types::DiscordAudioSample;
use crate::model::types::DiscordRtcTimestamp;
use crate::model::types::VoiceChannelEvent;

pub(crate) struct PacketHandler {
    ssrc_to_user_id: RwLock<std::collections::HashMap<types::Ssrc, types::UserId>>,
    tx_api_events: UnboundedSender<VoiceChannelEvent>,
    tx_audio_data: UnboundedSender<DiscordAudioData>,
    tx_voice_activity: UnboundedSender<UserAudioEvent>,
}

impl PacketHandler {
    pub(crate) async fn register(
        driver: Arc<tokio::sync::Mutex<songbird::Driver>>,
        tx_api_events: UnboundedSender<VoiceChannelEvent>,
        tx_audio_data: UnboundedSender<DiscordAudioData>,
        tx_voice_activity: UnboundedSender<UserAudioEvent>,
    ) {
        let handler = Self {
            ssrc_to_user_id: RwLock::new(std::collections::HashMap::new()),
            tx_api_events,
            tx_audio_data,
            tx_voice_activity,
        };
        register_events(handler, driver).await;
    }

    fn on_user_join(&self, ssrc: types::Ssrc, user_id: types::UserId) {
        {
            // map the SSRC to the user ID
            self.ssrc_to_user_id.write().unwrap().insert(ssrc, user_id);
        }
        self.tx_api_events
            .send(VoiceChannelEvent::UserJoin(user_id))
            .unwrap();
    }

    fn on_start_talking(&self, ssrc: types::Ssrc) {
        let user_id = self.user_id_from_ssrc(ssrc);
        if let Some(user_id) = user_id {
            self.tx_voice_activity
                .send(UserAudioEvent {
                    user_id,
                    event_type: UserAudioEventType::Speaking,
                })
                .unwrap();
        }
    }

    fn on_audio(
        &self,
        discord_audio: &[DiscordAudioSample],
        rtc_timestamp: DiscordRtcTimestamp,
        ssrc: types::Ssrc,
    ) {
        if let Some(user_id) = self.user_id_from_ssrc(ssrc) {
            self.tx_audio_data
                .send(DiscordAudioData {
                    user_id,
                    discord_audio: discord_audio.to_vec(),
                    rtc_timestamp,
                })
                .unwrap();
        }
    }

    /// Fired when a user stops talking.  Here, "stops talking" means
    /// the songbird driver has noticed 5 continuous packets (100ms) of silence.
    fn on_stop_talking(&self, ssrc: types::Ssrc) {
        if let Some(user_id) = self.user_id_from_ssrc(ssrc) {
            self.tx_voice_activity
                .send(UserAudioEvent {
                    user_id,
                    event_type: UserAudioEventType::Silent,
                })
                .unwrap();
        }
    }

    /// Fired when a user leaves the voice channel.
    fn on_user_leave(&self, user_id: types::UserId) {
        // we don't need to remove the SSRC from the map, since
        // if another user comes in with that SSRC, we'll just
        // overwrite the old user ID with the new one.
        //
        // Also, there's a chance that we might get other events
        // for this user after they leave, so we don't want to
        // remove them from the map just yet.
        self.tx_api_events
            .send(VoiceChannelEvent::UserLeave(user_id))
            .unwrap();
    }

    fn user_id_from_ssrc(&self, ssrc: types::Ssrc) -> Option<types::UserId> {
        self.ssrc_to_user_id.read().unwrap().get(&ssrc).copied()
    }
}

struct MyEventHandler<T>
where
    T: Fn(&songbird::EventContext, &Arc<PacketHandler>) + Send + Sync,
{
    handler: T,
    packet_handler: Arc<PacketHandler>,
}

#[async_trait]
impl<T> songbird::EventHandler for MyEventHandler<T>
where
    T: Fn(&songbird::EventContext, &Arc<PacketHandler>) + Send + Sync,
{
    async fn act(&self, ctx: &songbird::EventContext<'_>) -> Option<songbird::Event> {
        (self.handler)(ctx, &self.packet_handler);
        None
    }
}

pub(crate) async fn register_events(
    raw_handler: PacketHandler,
    driver: Arc<tokio::sync::Mutex<songbird::Driver>>,
) {
    let handler = Arc::new(raw_handler);
    // event handlers for the songbird driver
    driver.lock().await.add_global_event(
        songbird::CoreEvent::SpeakingStateUpdate.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::SpeakingStateUpdate(Speaking {
                    speaking,
                    ssrc,
                    user_id,
                    ..
                }) = ctx
                {
                    // user are assigned an SSRC when they start speaking.  We need
                    // to note this and map it to a user ID.

                    if speaking.microphone() {
                        // only look at users who are speaking using the microphone
                        // (the alternative is sharing their screen, which we ignore)

                        if let Some(user_id) = user_id {
                            my_handler.on_user_join(*ssrc, user_id.0);
                        } else {
                            eprintln!("No user_id for speaking state update");
                        }
                    }
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::SpeakingUpdate.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::SpeakingUpdate(SpeakingUpdateData { ssrc, speaking, .. }) = ctx
                {
                    // Called when a user starts or stops speaking.
                    if *speaking {
                        my_handler.on_start_talking(*ssrc);
                    } else {
                        my_handler.on_stop_talking(*ssrc);
                    }
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::VoicePacket.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::VoicePacket(VoiceData {
                    audio: Some(discord_audio),
                    packet,
                    ..
                }) = ctx
                {
                    // An event which fires for every received audio packet,
                    // containing the decoded data.
                    my_handler.on_audio(discord_audio, packet.timestamp.0, packet.ssrc);
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::ClientDisconnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::ClientDisconnect(
                    songbird::model::payload::ClientDisconnect { user_id, .. },
                ) = ctx
                {
                    // Called when a user leaves the voice channel.
                    my_handler.on_user_leave(user_id.0);
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::DriverConnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::DriverConnect(connect_data) = ctx {
                    my_handler
                        .tx_api_events
                        .send(VoiceChannelEvent::Connect(ConnectData::from(connect_data)))
                        .unwrap();
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::DriverDisconnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: move |ctx, my_handler| {
                if let EventContext::DriverDisconnect(disconnect_data) = ctx {
                    let result = my_handler.tx_api_events.send(VoiceChannelEvent::Disconnect(
                        DisconnectData::from(disconnect_data),
                    ));
                    if result.is_err() {
                        eprintln!("Disconnect event not sent (expected when exiting)");
                    }
                }
            },
        },
    );
    driver.lock().await.add_global_event(
        songbird::CoreEvent::DriverReconnect.into(),
        MyEventHandler {
            packet_handler: handler,
            handler: move |ctx, my_handler| {
                if let EventContext::DriverReconnect(connect_data) = ctx {
                    match my_handler.tx_api_events.send(VoiceChannelEvent::Reconnect(
                        ConnectData::from(connect_data),
                    )) {
                        Ok(_) => {} // everything is fine
                        Err(_) => {
                            eprintln!("Reconnect event not sent (expected when exiting)");
                        }
                    };
                }
            },
        },
    );
}
