use std::sync::Arc;

use audio::events::{DiscordAudioData, UserAudioEvent};
use audio::speaker::Speaker;
use audio::whisper::Whisper;
use model::constants::USER_SILENCE_TIMEOUT;
use model::types::VoiceChannelEvent;
use scrivening::manager::UserAudioManager;
use songbird::id::{ChannelId, GuildId, UserId};
use songbird::ConnectionInfo;
use songbird_client::packet_handler::PacketHandler;
use songbird_client::voice_activity::VoiceActivity;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod audio {
    pub(crate) mod audio_buffer;
    pub(crate) mod espeakng;
    pub(crate) mod events;
    pub(crate) mod resample;
    pub(crate) mod speaker;
    pub(crate) mod whisper;
}
pub mod model {
    pub(crate) mod constants;
    pub mod types;
}
mod scrivening {
    pub(crate) mod manager;
    pub(crate) mod worker;
}
mod songbird_client {
    pub(crate) mod packet_handler;
    pub(crate) mod voice_activity;
}
mod strategies {
    pub(crate) mod default_strategy;
    pub(crate) mod five_second_strategy;
    pub(crate) mod strategy_trait;
}

pub struct Discrivener {
    // task which will fire API change events
    api_task: Option<JoinHandle<()>>,
    audio_buffer_manager_task: Option<JoinHandle<()>>,
    // use tokio mutex as it's held during an .await
    driver: Arc<tokio::sync::Mutex<songbird::Driver>>,
    shutdown_token: CancellationToken,
    speaker: Option<JoinHandle<()>>,
    tx_speaker: tokio::sync::mpsc::UnboundedSender<String>,
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
            tokio::sync::mpsc::unbounded_channel::<UserAudioEvent>();
        let (tx_speaker, rx_speaker) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (tx_voice_activity, rx_voice_activity) =
            tokio::sync::mpsc::unbounded_channel::<UserAudioEvent>();

        let voice_activity_task = Some(VoiceActivity::monitor(
            rx_voice_activity,
            shutdown_token.clone(),
            tx_api_events.clone(),
            tx_silent_user_events,
            USER_SILENCE_TIMEOUT,
        ));

        let whisper = Whisper::load(model_path);

        // the audio buffer manager gets the voice data
        let audio_buffer_manager_task = Some(UserAudioManager::monitor(
            rx_audio_data,
            rx_silent_user_events,
            shutdown_token.clone(),
            tx_api_events.clone(),
            whisper,
        ));

        let driver = Arc::new(tokio::sync::Mutex::new(songbird::Driver::new(config)));
        PacketHandler::register(
            driver.clone(),
            tx_api_events,
            tx_audio_data,
            tx_voice_activity,
        )
        .await;

        let api_task = Some(tokio::spawn(Self::start_api_task(
            rx_api_events,
            shutdown_token.clone(),
            event_callback,
        )));

        let speaker = Some(Speaker::monitor(
            driver.clone(),
            rx_speaker,
            shutdown_token.clone(),
        ));

        Self {
            api_task,
            audio_buffer_manager_task,
            driver,
            shutdown_token,
            speaker,
            tx_speaker,
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
        self.driver.lock().await.connect(connection_info).await
    }

    pub async fn disconnect(&mut self) {
        self.driver.try_lock().unwrap().stop();
        self.driver.try_lock().unwrap().leave();
        self.shutdown_token.cancel();

        // join all our tasks
        self.api_task.take().unwrap().await.unwrap();
        self.audio_buffer_manager_task
            .take()
            .unwrap()
            .await
            .unwrap();
        self.speaker.take().unwrap().await.unwrap();
        self.voice_activity_task.take().unwrap().await.unwrap();
    }

    async fn start_api_task(
        mut rx_api_events: tokio::sync::mpsc::UnboundedReceiver<VoiceChannelEvent>,
        shutdown_token: CancellationToken,
        event_callback: std::sync::Arc<dyn Fn(VoiceChannelEvent) + Send + Sync>,
    ) {
        // wait for either shutdown token or rx_api_events
        loop {
            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    return;
                }
                Some(event) = rx_api_events.recv() => {
                    event_callback(event);
                }
            }
        }
    }

    pub fn speak(&mut self, message: String) {
        self.tx_speaker.send(message).unwrap();
    }
}
