use crate::api::api_types;
use crate::model::types;
use std::collections;
use tokio::sync;
use tokio::task;
use tokio::time;

struct VoiceActivity(collections::HashMap<types::UserId, (bool, time::Instant)>);

/// Input:
///  - ssid_starts_talking event
///  - ssid_stops_talking event
/// Output:
///  - answers queries about:
///    - is_anyone_talking?
///    - when was our most recent audio from SSID?
impl VoiceActivity {
    pub fn monitor(
        rxQueue: sync::watch::Receiver<api_types::VoiceActivityData>,
    ) -> task::JoinHandle<()> {
        let voice_activity = Self(collections::HashMap::new());
        task::spawn(async move {
            while rxQueue.changed().await.is_ok() {
                let activity = *rxQueue.borrow();
                voice_activity
                    .0
                    .insert(activity.user_id, (activity.speaking, time::Instant::now()));
            }
        })
    }

    pub fn is_anyone_talking(&self) -> bool {
        self.0.iter().any(|(_, (speaking, _))| *speaking)
    }

    pub fn last_audio_instant(&self, ssid: types::UserId) -> Option<time::Instant> {
        self.0.get(&ssid).map(|(_, instant)| *instant)
    }
}
