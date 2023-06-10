use crate::api::api_types;
use std::collections;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync;
use tokio::task;
use tokio::time;

struct VoiceActivity(collections::HashMap<api_types::UserId, (bool, time::Instant)>);

/// Input:
///  - user_id_starts_talking event
///  - user_id_stops_talking event
/// Output:
///  - answers queries about:
///    - is_anyone_talking?
///    - when was our most recent audio from user_id?
impl VoiceActivity {
    pub async fn monitor(
        mut rx_queue: sync::mpsc::UnboundedReceiver<api_types::VoiceActivityData>,
    ) -> (task::JoinHandle<()>, Arc<Mutex<VoiceActivity>>) {
        let voice_activity = Arc::new(Mutex::new(Self(collections::HashMap::new())));
        let voice_activity_clone = voice_activity.clone();
        let join_handle = task::spawn(async move {
            while let Some(activity) = rx_queue.recv().await {
                voice_activity_clone
                    .lock()
                    .unwrap()
                    .0
                    .insert(activity.user_id, (activity.speaking, time::Instant::now()));
            }
        });
        return (join_handle, voice_activity);
    }

    pub fn is_anyone_talking(&self) -> bool {
        self.0.iter().any(|(_, (speaking, _))| *speaking)
    }

    pub fn latest_audio_instant(&self, user_id: api_types::UserId) -> Option<time::Instant> {
        self.0.get(&user_id).map(|(_, instant)| *instant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_voice_activity() {
        let (tx_queue, rx_queue) = sync::mpsc::unbounded_channel();
        let (handle, activity_monitor) = super::VoiceActivity::monitor(rx_queue).await;
        assert! {
            !activity_monitor.lock().unwrap().is_anyone_talking()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(1).is_none()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(2).is_none()
        };

        tx_queue
            .send(api_types::VoiceActivityData {
                user_id: 1,
                speaking: true,
            })
            .unwrap();

        // sleep slightly to allow the monitor to process the event
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert! {
            activity_monitor.lock().unwrap().is_anyone_talking()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(1).is_some()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(2).is_none()
        };

        tx_queue
            .send(api_types::VoiceActivityData {
                user_id: 2,
                speaking: true,
            })
            .unwrap();

        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 1,
                speaking: false,
            })
            .unwrap();

        // sleep slightly to allow the monitor to process the event
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert! {
            activity_monitor.lock().unwrap().is_anyone_talking()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(1).is_some()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(2).is_some()
        };

        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 2,
                speaking: false,
            })
            .unwrap();
        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 1,
                speaking: true,
            })
            .unwrap();
        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 2,
                speaking: true,
            })
            .unwrap();

        // sleep slightly to allow the monitor to process the event
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert! {
            activity_monitor.lock().unwrap().is_anyone_talking()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(1).is_some()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(2).is_some()
        };

        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 1,
                speaking: false,
            })
            .unwrap();
        tx_queue
            .send(super::api_types::VoiceActivityData {
                user_id: 2,
                speaking: false,
            })
            .unwrap();

        drop(tx_queue);
        handle.await.unwrap();

        assert! {
            !activity_monitor.lock().unwrap().is_anyone_talking()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(1).is_some()
        };
        assert! {
            activity_monitor.lock().unwrap().latest_audio_instant(2).is_some()
        };
    }
}
