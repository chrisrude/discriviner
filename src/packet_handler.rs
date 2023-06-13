use std::sync::Arc;

/// Manages multiple buffers for each user who is speaking.
/// Tracks when users have stopped speaking, and fires a callback.
use async_trait::async_trait;

use songbird::events::context_data::ConnectData;
use songbird::events::context_data::DisconnectData;
use songbird::events::context_data::DisconnectKind;
use songbird::events::context_data::DisconnectReason;
use songbird::events::context_data::SpeakingUpdateData;
use songbird::events::context_data::VoiceData;
use songbird::model::payload::Speaking;
use songbird::EventContext;

use std::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::api_types;
use crate::events::audio::DiscordVoiceData;
use crate::events::audio::VoiceActivityData;
use crate::model::types;
use crate::model::types::DiscordAudioSample;
use crate::model::types::DiscordRtcTimestamp;

pub(crate) struct PacketHandler {
    ssrc_to_user_id: RwLock<std::collections::HashMap<types::Ssrc, types::UserId>>,
    audio_events_sender: UnboundedSender<DiscordVoiceData>,
    user_api_events_sender: UnboundedSender<api_types::VoiceChannelEvent>,
    voice_activity_events_sender: UnboundedSender<VoiceActivityData>,
}

impl PacketHandler {
    pub(crate) async fn new(
        driver: &mut songbird::Driver,
        audio_events_sender: UnboundedSender<DiscordVoiceData>,
        user_api_events_sender: UnboundedSender<api_types::VoiceChannelEvent>,
        voice_activity_events_sender: UnboundedSender<VoiceActivityData>,
    ) -> Arc<Self> {
        let handler = Arc::new(Self {
            ssrc_to_user_id: RwLock::new(std::collections::HashMap::new()),
            audio_events_sender,
            user_api_events_sender,
            voice_activity_events_sender,
        });
        register_events(&handler, driver).await;
        handler
    }

    fn on_user_join(&self, ssrc: types::Ssrc, user_id: types::UserId) {
        {
            // map the SSRC to the user ID
            self.ssrc_to_user_id.write().unwrap().insert(ssrc, user_id);
        }
        self.user_api_events_sender
            .send(api_types::VoiceChannelEvent::UserJoin(
                api_types::UserJoinData {
                    user_id,
                    joined: true,
                },
            ))
            .unwrap();
    }

    fn on_start_talking(&self, ssrc: types::Ssrc) {
        let user_id = self.user_id_from_ssrc(ssrc);
        if let Some(user_id) = user_id {
            self.voice_activity_events_sender
                .send(VoiceActivityData {
                    user_id,
                    speaking: true,
                })
                .unwrap();
        }
    }

    fn on_audio(
        &self,
        audio: &Vec<DiscordAudioSample>,
        timestamp: DiscordRtcTimestamp,
        ssrc: types::Ssrc,
    ) {
        if let Some(user_id) = self.user_id_from_ssrc(ssrc) {
            self.audio_events_sender
                .send(DiscordVoiceData {
                    user_id,
                    audio: audio.clone(),
                    timestamp,
                })
                .unwrap();
        }
    }

    /// Fired when a user stops talking.  Here, "stops talking" means
    /// the songbird driver has noticed 5 continuous packets (100ms) of silence.
    fn on_stop_talking(&self, ssrc: types::Ssrc) {
        if let Some(user_id) = self.user_id_from_ssrc(ssrc) {
            self.voice_activity_events_sender
                .send(VoiceActivityData {
                    user_id,
                    speaking: false,
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
        self.user_api_events_sender
            .send(api_types::VoiceChannelEvent::UserJoin(
                api_types::UserJoinData {
                    user_id,
                    joined: false,
                },
            ))
            .unwrap();
    }

    fn on_driver_connect(&self, connect_data: api_types::ConnectData) {
        self.user_api_events_sender
            .send(api_types::VoiceChannelEvent::Connect(connect_data))
            .unwrap();
    }

    fn on_driver_disconnect(&self, disconnect_data: api_types::DisconnectData) {
        self.user_api_events_sender
            .send(api_types::VoiceChannelEvent::Disconnect(disconnect_data))
            .unwrap();
    }

    fn on_driver_reconnect(&self, connect_data: api_types::ConnectData) {
        self.user_api_events_sender
            .send(api_types::VoiceChannelEvent::Reconnect(connect_data))
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

impl<T> MyEventHandler<T>
where
    T: Fn(&songbird::EventContext, &Arc<PacketHandler>) + Send + Sync,
{
    fn new(packet_handler: Arc<PacketHandler>, handler: T) -> Self {
        Self {
            packet_handler,
            handler,
        }
    }
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

pub(crate) async fn register_events(handler: &Arc<PacketHandler>, driver: &mut songbird::Driver) {
    // event handlers for the songbird driver
    driver.add_global_event(
        songbird::CoreEvent::SpeakingStateUpdate.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
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
    driver.add_global_event(
        songbird::CoreEvent::SpeakingUpdate.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
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
    driver.add_global_event(
        songbird::CoreEvent::VoicePacket.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
                if let EventContext::VoicePacket(VoiceData { audio, packet, .. }) = ctx {
                    // An event which fires for every received audio packet,
                    // containing the decoded data.
                    if let Some(audio) = audio {
                        my_handler.on_audio(audio, packet.timestamp.0, packet.ssrc);
                    }
                }
            },
        },
    );
    driver.add_global_event(
        songbird::CoreEvent::ClientDisconnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
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
    driver.add_global_event(
        songbird::CoreEvent::DriverConnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
                if let EventContext::DriverConnect(ConnectData {
                    guild_id,
                    channel_id,
                    server,
                    session_id,
                    ..
                }) = ctx
                {
                    my_handler.on_driver_connect(api_types::ConnectData {
                        guild_id: guild_id.0,
                        channel_id: channel_id.map(|c| c.0),
                        session_id: session_id.to_string(),
                        server: server.to_string(),
                    })
                }
            },
        },
    );
    driver.add_global_event(
        songbird::CoreEvent::DriverDisconnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
                if let EventContext::DriverDisconnect(DisconnectData {
                    kind,
                    reason,
                    channel_id,
                    guild_id,
                    session_id,
                    ..
                }) = ctx
                {
                    my_handler.on_driver_disconnect(api_types::DisconnectData {
                        kind: api_types::DisconnectKind::from(*kind),
                        reason: reason.map(|r| api_types::DisconnectReason::from(r)),
                        channel_id: channel_id.map(|c| c.0),
                        guild_id: guild_id.0,
                        session_id: session_id.to_string(),
                    })
                }
            },
        },
    );
    driver.add_global_event(
        songbird::CoreEvent::DriverReconnect.into(),
        MyEventHandler {
            packet_handler: handler.clone(),
            handler: |ctx, my_handler| {
                if let EventContext::DriverReconnect(ConnectData {
                    guild_id,
                    channel_id,
                    server,
                    session_id,
                    ..
                }) = ctx
                {
                    my_handler.on_driver_reconnect(api_types::ConnectData {
                        guild_id: guild_id.0,
                        channel_id: channel_id.map(|c| c.0),
                        session_id: session_id.to_string(),
                        server: server.to_string(),
                    })
                }
            },
        },
    );
}

impl api_types::DisconnectKind {
    fn from(kind: DisconnectKind) -> api_types::DisconnectKind {
        match kind {
            DisconnectKind::Connect => api_types::DisconnectKind::Connect,
            DisconnectKind::Reconnect => api_types::DisconnectKind::Reconnect,
            DisconnectKind::Runtime => api_types::DisconnectKind::Runtime,
            _ => api_types::DisconnectKind::Unknown,
        }
    }
}

impl api_types::DisconnectReason {
    fn from(reason: DisconnectReason) -> api_types::DisconnectReason {
        match reason {
            DisconnectReason::AttemptDiscarded => api_types::DisconnectReason::AttemptDiscarded,
            DisconnectReason::Internal => api_types::DisconnectReason::Internal,
            DisconnectReason::Io => api_types::DisconnectReason::Io,
            DisconnectReason::ProtocolViolation => api_types::DisconnectReason::ProtocolViolation,
            DisconnectReason::TimedOut => api_types::DisconnectReason::TimedOut,
            DisconnectReason::WsClosed(code) => {
                api_types::DisconnectReason::WsClosed(code.map(|c| c as u32))
            }
            _ => api_types::DisconnectReason::Unknown,
        }
    }
}
