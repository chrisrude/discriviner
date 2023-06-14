use std::sync::Arc;
use std::time::Duration;

use crate::api::api_types;
use crate::events::audio::{ConversionRequest, DiscordAudioData, VoiceActivityData};
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
    handler: Arc<packet_handler::PacketHandler>,
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

        let (audio_events_sender, audio_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<DiscordAudioData>();
        let (user_api_events_sender, user_api_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<api_types::VoiceChannelEvent>();
        let (voice_activity_events_sender, voice_activity_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<VoiceActivityData>();

        let (conversion_requests_sender, conversion_requests_receiver) =
            tokio::sync::mpsc::unbounded_channel::<ConversionRequest>();

        let (silent_user_events_sender, silent_user_events_receiver) =
            tokio::sync::mpsc::unbounded_channel::<u64>();

        let voice_activity_task = voice_activity::VoiceActivity::monitor(
            voice_activity_events_receiver,
            Duration::from_millis(types::USER_SILENCE_TIMEOUT_MS),
            user_api_events_sender.clone(),
            silent_user_events_sender,
        );

        // the audio buffer manager gets the voice data
        let audio_buffer_manager_task = crate::model::audio_buffer::AudioBufferManager::monitor(
            audio_events_receiver,
            silent_user_events_receiver,
            conversion_requests_sender,
        );

        let mut driver = songbird::Driver::new(config);

        let handler = packet_handler::PacketHandler::new(
            &mut driver,
            audio_events_sender,
            user_api_events_sender,
            voice_activity_events_sender,
        )
        .await;

        let whisper_task =
            whisper::Whisper::load_and_monitor(model_path, conversion_requests_receiver);

        let api_task = tokio::spawn(Self::start_api_task(
            user_api_events_receiver,
            event_callback,
        ));

        Self {
            api_task,
            audio_buffer_manager_task,
            driver,
            handler,
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
        mut user_api_events_receiver: tokio::sync::mpsc::UnboundedReceiver<
            api_types::VoiceChannelEvent,
        >,
        event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
    ) {
        while let Some(event) = user_api_events_receiver.recv().await {
            event_callback(event);
        }
    }
}
