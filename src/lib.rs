use crate::events::audio::{DiscordAudioData, VoiceActivityData};
use crate::model::audio_manager::AudioManager;

use model::constants::USER_SILENCE_TIMEOUT;
use model::types::VoiceChannelEvent;
use model::voice_activity::VoiceActivity;
use model::whisper::Whisper;
use songbird::id::{ChannelId, GuildId, UserId};
use songbird::ConnectionInfo;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod events {
    pub(crate) mod audio;
}
pub mod model {
    pub(crate) mod audio_manager;
    pub(crate) mod audio_slice;
    pub(crate) mod constants;
    pub mod types;
    pub(crate) mod user_audio;
    pub(crate) mod voice_activity;
    pub(crate) mod whisper;
}
mod packet_handler;

pub struct Discrivener {
    // task which will fire API change events
    api_task: Option<JoinHandle<()>>,
    audio_buffer_manager_task: Option<JoinHandle<()>>,
    driver: songbird::Driver,
    shutdown_token: CancellationToken,
    voice_activity_task: Option<JoinHandle<()>>,
}

impl Discrivener {
    pub async fn load(
        model_path: String,
        event_callback: std::sync::Arc<dyn Fn(VoiceChannelEvent) + Send + Sync>,
    ) -> Self {
        let mut config = songbird::Config::default();
        config.decode_mode = songbird::driver::DecodeMode::Decode; // convert incoming audio from Opus to PCM

        let shutdown_token = CancellationToken::new();
        let (tx_audio_data, rx_audio_data) =
            tokio::sync::mpsc::unbounded_channel::<DiscordAudioData>();
        let (tx_api_events, rx_api_events) =
            tokio::sync::mpsc::unbounded_channel::<VoiceChannelEvent>();
        let (tx_silent_user_events, rx_silent_user_events) =
            tokio::sync::mpsc::unbounded_channel::<u64>();
        let (tx_voice_activity, rx_voice_activity) =
            tokio::sync::mpsc::unbounded_channel::<VoiceActivityData>();

        let voice_activity_task = Some(VoiceActivity::monitor(
            rx_voice_activity,
            shutdown_token.clone(),
            tx_api_events.clone(),
            tx_silent_user_events,
            USER_SILENCE_TIMEOUT,
        ));

        let whisper = Whisper::load(model_path);

        // the audio buffer manager gets the voice data
        let audio_buffer_manager_task = Some(AudioManager::monitor(
            rx_audio_data,
            rx_silent_user_events,
            shutdown_token.clone(),
            tx_api_events.clone(),
            whisper,
        ));

        let mut driver = songbird::Driver::new(config);
        packet_handler::PacketHandler::register(
            &mut driver,
            tx_api_events,
            tx_audio_data,
            tx_voice_activity,
        );

        let api_task = Some(tokio::spawn(Self::start_api_task(
            rx_api_events,
            event_callback,
        )));

        Self {
            api_task,
            audio_buffer_manager_task,
            driver,
            shutdown_token,
            voice_activity_task,
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

    pub async fn disconnect(&mut self) {
        self.driver.leave();
        self.shutdown_token.cancel();

        // join all our tasks
        self.api_task.take().unwrap().await.unwrap();
        self.audio_buffer_manager_task
            .take()
            .unwrap()
            .await
            .unwrap();
        self.voice_activity_task.take().unwrap().await.unwrap();
    }

    async fn start_api_task(
        mut rx_api_events: tokio::sync::mpsc::UnboundedReceiver<VoiceChannelEvent>,
        event_callback: std::sync::Arc<dyn Fn(VoiceChannelEvent) + Send + Sync>,
    ) {
        while let Some(event) = rx_api_events.recv().await {
            event_callback(event);
        }
    }
}
