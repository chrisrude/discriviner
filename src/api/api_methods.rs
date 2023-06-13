#![allow(unused_variables)]
use std::sync::Arc;

use crate::api::api_types;
use crate::events::audio::{DiscordVoiceData, VoiceActivityData};
use crate::model::whisper;
use crate::packet_handler;

use songbird::id::{ChannelId, GuildId, UserId};
use songbird::ConnectionInfo;

pub struct Discrivener {
    driver: songbird::Driver,
    event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
    handler: Arc<packet_handler::PacketHandler>,
    whisper: whisper::Whisper,
}

impl Discrivener {
    pub async fn load(
        model_path: String,
        event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
    ) -> Self {
        let mut config = songbird::Config::default();
        config.decode_mode = songbird::driver::DecodeMode::Decode; // convert incoming audio from Opus to PCM

        let (audio_events_sender, audio_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<DiscordVoiceData>();
        let (user_api_events_sender, user_api_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<api_types::VoiceChannelEvent>();
        let (voice_activity_events_sender, voice_activity_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<VoiceActivityData>();

        // the audio buffer manager gets the voice data
        let audio_buffer_manager =
            crate::model::audio_buffer::AudioBufferManager::monitor(audio_events_receiver);

        let mut driver = songbird::Driver::new(config);

        let handler = packet_handler::PacketHandler::new(
            &mut driver,
            audio_events_sender,
            user_api_events_sender,
            voice_activity_events_sender,
        )
        .await;

        let whisper = whisper::Whisper::load(model_path);

        Self {
            driver,
            event_callback,
            handler,
            whisper,
        }
    }

    /// Connect to a voice channel.
    ///
    /// channel_id:
    /// ID of the voice channel to join
    ///
    /// This is not needed to establish a connection, but can be useful
    /// for book-keeping.
    /// URL of the voice websocket gateway server assigned to this call.
    /// ID of the target voice channel's parent guild.
    ///
    /// Bots cannot connect to a guildless (i.e., direct message) voice call.
    /// Unique string describing this session for validation/authentication purposes.
    /// Ephemeral secret used to validate the above session.
    /// UserID of this bot.
    pub async fn connect(
        &mut self,
        channel_id: u64,
        endpoint: &str,
        guild_id: u64,
        session_id: &str,
        user_id: u64,
        voice_token: &str,
    ) -> Result<(), songbird::error::ConnectionError> {
        let connection_info = ConnectionInfo {
            channel_id: Some(ChannelId::from(channel_id)),
            endpoint: endpoint.to_string(),
            guild_id: GuildId::from(guild_id),
            session_id: session_id.to_string(),
            token: voice_token.to_string(),
            user_id: UserId::from(user_id),
        };
        self.driver.connect(connection_info).await
    }

    pub fn disconnect(&mut self) {
        self.driver.leave();
    }
}
