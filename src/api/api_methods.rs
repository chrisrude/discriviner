
use std::time::Duration;

use crate::api::api_types;
use crate::events::audio::{DiscordAudioData, TranscriptionRequest, VoiceActivityData};
use crate::model::{types, voice_activity, whisper};
use crate::packet_handler;

use songbird::id::{ChannelId, GuildId, UserId};
use songbird::ConnectionInfo;
use tokio::task::JoinHandle;

pub struct Discrivener {
    // task which will fire API change events
    api_task: JoinHandle<()>,
    audio_buffer_manager_task: JoinHandle<()>,
    driver: songbird::Driver,
    whisper_task: JoinHandle<()>,
    voice_activity_task: JoinHandle<()>,
}

impl Discrivener {
    pub async fn load(
        model_path: String,
        event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
    ) -> Self {
        let mut config = songbird::Config::default();
        config.decode_mode = songbird::driver::DecodeMode::Decode; // convert incoming audio from Opus to PCM

        // todo: broadcast shutdown event

        let (tx_audio_data, rx_audio_data) =
            tokio::sync::mpsc::unbounded_channel::<DiscordAudioData>();
        let (tx_api_events, rx_api_events) =
            tokio::sync::mpsc::unbounded_channel::<api_types::VoiceChannelEvent>();
        let (tx_silent_user_events, rx_silent_user_events) =
            tokio::sync::mpsc::unbounded_channel::<u64>();
        let (tx_transcription_requests, rx_transcription_requests) =
            tokio::sync::mpsc::unbounded_channel::<TranscriptionRequest>();
        let (tx_voice_activity, rx_voice_activity) =
            tokio::sync::mpsc::unbounded_channel::<VoiceActivityData>();

        let voice_activity_task = voice_activity::VoiceActivity::monitor(
            rx_voice_activity,
            Duration::from_millis(types::USER_SILENCE_TIMEOUT_MS),
            tx_api_events.clone(),
            tx_silent_user_events,
        );

        // the audio buffer manager gets the voice data
        let audio_buffer_manager_task = crate::model::audio_buffer::AudioBufferManager::monitor(
            rx_audio_data,
            rx_silent_user_events,
            tx_transcription_requests,
        );

        let mut driver = songbird::Driver::new(config);
        packet_handler::PacketHandler::register(
            &mut driver,
            tx_api_events,
            tx_audio_data,
            tx_voice_activity,
        );

        let whisper_task =
            whisper::Whisper::load_and_monitor(model_path, rx_transcription_requests);

        let api_task = tokio::spawn(Self::start_api_task(rx_api_events, event_callback));

        Self {
            api_task,
            audio_buffer_manager_task,
            driver,
            voice_activity_task,
            whisper_task,
        }
    }

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

        // todo: shutdown queues and workers
    }

    async fn start_api_task(
        mut rx_api_events: tokio::sync::mpsc::UnboundedReceiver<api_types::VoiceChannelEvent>,
        event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
    ) {
        while let Some(event) = rx_api_events.recv().await {
            event_callback(event);
        }
    }
}
