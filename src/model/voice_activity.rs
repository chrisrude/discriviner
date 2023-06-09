use crate::api::api_types;
use crate::model::types;
use std::collections;
use tokio::sync;

struct VoiceActivity {
    map collections::HashMap<types::Ssrc, api_types::VoiceActivityData>;
}

/// Input:
///  - ssid_starts_talking event
///  - ssid_stops_talking event
/// Output:
///  - answers queries about:
///    - is_anyone_talking?
///    - when was our most recent audio from SSID?
impl VoiceActivity {
    pub fn new() -> Self {
        Self(collections::HashMap::new())
    }

    pub fn on_speaking_update(&mut self, data: api_types::SpeakingUpdateData) {
        let user_id = data.user_id;
        let ssrc = data.ssrc;

        let entry = self.0.entry(ssrc).or_insert(api_types::VoiceActivityData {
            user_id,
            speaking: false,
            ssrc,
            start_time: None,
            end_time: None,
        });

        entry.speaking = data.speaking;
        if entry.start_time.is_none() {
            entry.start_time = Some(data.timestamp);
        }
    }
}
