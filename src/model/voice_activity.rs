use crate::api::api_types;
use crate::api::api_types::UserId;
use futures::stream::futures_unordered::FuturesUnordered;
use std::collections;
use std::thread::JoinHandle;
use tokio::sync;
use tokio::task;
use tokio::time;

struct VoiceActivity {
    last_time_by_user: collections::HashMap<api_types::UserId, (bool, time::Instant)>,
    deferred_silence_callbacks: FuturesUnordered<JoinHandle<()>>,
    rx_queue: sync::mpsc::UnboundedReceiver<api_types::VoiceActivityData>,
    tx_queue_silent_channel: sync::mpsc::UnboundedSender<bool>,
    tx_queue_silent_user: sync::mpsc::UnboundedSender<UserId>,
    user_silence_timeout: std::time::Duration,
}

/// Input:
///  - user_id_starts_talking event
///  - user_id_stops_talking event
/// Output:
///  - emits events when:
///    - all users stop talking
///    - anyone then starts talking
///  - after a user has been silent for N seconds
impl VoiceActivity {
    pub async fn monitor(
        rx_queue: sync::mpsc::UnboundedReceiver<api_types::VoiceActivityData>,
        user_silence_timeout: time::Duration,
        tx_queue_silent_channel: sync::mpsc::UnboundedSender<bool>,
        tx_queue_silent_user: sync::mpsc::UnboundedSender<UserId>,
    ) -> task::JoinHandle<()> {
        let mut voice_activity = Self {
            last_time_by_user: collections::HashMap::new(),
            deferred_silence_callbacks: FuturesUnordered::new(),
            rx_queue,
            tx_queue_silent_channel,
            tx_queue_silent_user,
            user_silence_timeout,
        };
        task::spawn(async move {
            voice_activity.loop_forever().await;
        })
    }

    async fn loop_forever(&mut self) {
        loop {
            tokio::select! {
                maybe_activity = self.rx_queue.recv() => {
                    if maybe_activity.is_none() {
                        // channel closed
                        break;
                    }
                    let activity = maybe_activity.unwrap();
                    let now = time::Instant::now();
                    let silence_interval_end = now + self.user_silence_timeout;

                    // store the last time we heard from this user
                    self.last_time_by_user
                        .insert(activity.user_id, (activity.speaking, now));

                    // start a simple task that will fire a callback when
                    // the user has been silent for N seconds.  In that callback,
                    // we check to see if the user has been silent the whole time.
                    // If so, we emit a "user stopped talking" event.
                    // If the user has any voice activity since then, we just do nothing.
                    let user_id = activity.user_id;
                    let monitor_task = task::spawn(async move {
                        time::sleep_until(silence_interval_end).await;
                        return user_id;
                    });
                }
            }
        }
        // cleanup: cancel pending silence callbacks.
        // this might not be necessary, since the callbacks will be dropped
        // anyway, but it's good to be explicit.
        self.deferred_silence_callbacks.clear();
    }

    // // check to see if the user has been silent the whole time
    // if let Some((speaking, last_time)) = self.last_time_by_user.get(&user_id) {
    //     if !*speaking && *last_time == now {
    //         // user has been silent the whole time
    //         self.tx_queue_silent_user.send(user_id).unwrap();
    //     }
    // }

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
