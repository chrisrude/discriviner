use audio::events::UserAudioEvent;
use audio::whisper::Whisper;
use model::constants::USER_SILENCE_TIMEOUT;
use model::types::VoiceChannelEvent;
use scrivening::manager::UserAudioEventDemux;
use songbird::id::{ChannelId, GuildId, UserId};
use songbird::ConnectionInfo;
use songbird_client::packet_handler::PacketHandler;
use songbird_client::voice_activity::VoiceActivity;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod audio {
    pub(crate) mod audio_buffer;
    pub(crate) mod events;
    pub(crate) mod whisper;
}
pub mod model {
    pub(crate) mod constants;
    pub mod types;
}
mod scrivening {
    pub(crate) mod manager;
    pub(crate) mod transcript_director;
}
mod songbird_client {
    pub(crate) mod packet_handler;
    pub(crate) mod voice_activity;
}

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
            tokio::sync::mpsc::unbounded_channel::<UserAudioEvent>();
        let (tx_api_events, rx_api_events) =
            tokio::sync::mpsc::unbounded_channel::<VoiceChannelEvent>();
        let (tx_silent_user_events, rx_silent_user_events) =
            tokio::sync::mpsc::unbounded_channel::<UserAudioEvent>();
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
        let audio_buffer_manager_task = Some(UserAudioEventDemux::monitor(
            rx_audio_data,
            rx_silent_user_events,
            shutdown_token.clone(),
            tx_api_events.clone(),
            whisper,
        ));

        let mut driver = songbird::Driver::new(config);
        PacketHandler::register(&mut driver, tx_api_events, tx_audio_data, tx_voice_activity);

        let api_task = Some(tokio::spawn(Self::start_api_task(
            rx_api_events,
            shutdown_token.clone(),
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
        eprintln!("Driver leaving");
        self.driver.leave();
        eprintln!("Setting shutdown token");
        self.shutdown_token.cancel();

        // join all our tasks
        eprintln!("Waiting for tasks to finish");
        self.api_task.take().unwrap().await.unwrap();
        eprintln!("API task complete");
        self.audio_buffer_manager_task
            .take()
            .unwrap()
            .await
            .unwrap();
        eprintln!("Audio buffer manager task complete");
        self.voice_activity_task.take().unwrap().await.unwrap();
        eprintln!("Voice activity task complete");
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
}
