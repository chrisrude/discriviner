#![allow(unused_variables)]
use std::sync::Arc;

use crate::api::api_types;
use crate::events::audio::{DiscordVoiceData, VoiceActivityData};
use crate::model::whisper;
use crate::packet_handler;

/// Receives audio from Discord, and sends it to the Whisper model.
pub struct Model {
    driver: songbird::Driver,
    handler: Arc<packet_handler::PacketHandler>,
    whisper: whisper::Whisper,
}

impl Model {
    pub async fn load(model_path: String) -> Self {
        let mut config = songbird::Config::default();
        config.decode_mode = songbird::driver::DecodeMode::Decode; // convert incoming audio from Opus to PCM

        let (audio_events_sender, audio_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<DiscordVoiceData>();
        let (user_api_events_sender, user_api_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<api_types::VoiceChannelEvent>();
        let (voice_activity_events_sender, voice_activity_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<VoiceActivityData>();

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
            handler,
            whisper,
        }
    }

    pub async fn connect(
        &mut self,
        connection_info: songbird::ConnectionInfo,
    ) -> Result<(), songbird::error::ConnectionError> {
        self.driver.connect(connection_info).await
    }

    pub fn disconnect(&mut self) {
        self.driver.leave();
    }
}
