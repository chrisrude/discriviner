use crate::events::audio::VoiceActivityData;
use crate::model::types::UserId;
use std::collections;
use std::collections::VecDeque;
use tokio::sync;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio::time::Instant;

struct VoiceActivity {
    last_time_by_user: collections::HashMap<UserId, (bool, time::Instant)>,
    silence_list: VecDeque<(Instant, UserId, Instant)>,
    rx_queue: sync::mpsc::UnboundedReceiver<VoiceActivityData>,
    speaking_users: collections::HashSet<UserId>,
    tx_queue_silent_channel: sync::mpsc::UnboundedSender<bool>,
    tx_queue_silent_user: sync::mpsc::UnboundedSender<UserId>,
    user_silence_timeout: Duration,
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
    const ONE_YEAR: Duration = Duration::from_secs(60 * 60 * 24 * 365);

    pub async fn monitor(
        rx_queue: sync::mpsc::UnboundedReceiver<VoiceActivityData>,
        user_silence_timeout: Duration,
        tx_queue_silent_channel: sync::mpsc::UnboundedSender<bool>,
        tx_queue_silent_user: sync::mpsc::UnboundedSender<UserId>,
    ) -> task::JoinHandle<()> {
        let mut voice_activity = Self {
            last_time_by_user: collections::HashMap::new(),
            silence_list: VecDeque::new(),
            speaking_users: collections::HashSet::new(),
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
            let forever = time::Instant::now().checked_add(Self::ONE_YEAR).unwrap();
            let noop_time = (forever, 0, forever);

            // next future will always be at the end... only need to wait for a single one ever
            tokio::select! {
                maybe_activity = self.rx_queue.recv() => {
                    if maybe_activity.is_none() {
                        // the sender has been dropped, so we are done
                        return;
                    }
                    let activity = maybe_activity.unwrap();
                    let now = time::Instant::now();

                    // store the last time we heard from this user.  If the user
                    // has not spoken again at the end of this interval, then
                    // we will send a silent user event
                    self.last_time_by_user
                        .insert(activity.user_id, (activity.speaking, now));

                    self.silence_list.push_back((now + self.user_silence_timeout, activity.user_id, now));

                    // track the list of users speaking in the channel, and
                    // send an event when the list goes from empty to non-empty
                    // or from non-empty to empty
                    if activity.speaking {
                        let was_empty = self.speaking_users.is_empty();
                        self.speaking_users.insert(activity.user_id);
                        if was_empty {
                            self.tx_queue_silent_channel.send(false).unwrap();
                        }
                    } else {
                        let was_someone_speaking = !self.speaking_users.is_empty();
                        self.speaking_users.remove(&activity.user_id);
                        if was_someone_speaking && self.speaking_users.is_empty() {
                            self.tx_queue_silent_channel.send(true).unwrap();
                        }
                    }
                }
                () = time::sleep_until(
                    self.silence_list.get(0).unwrap_or(&noop_time).0
                ) => {
                    let now = time::Instant::now();
                    let mut silence_list = std::mem::take(&mut self.silence_list);
                    while let Some((silence_timeout, user_id, last_time)) = silence_list.pop_front() {
                        if silence_timeout > now {
                            silence_list.push_front((silence_timeout, user_id, last_time));
                            break;
                        }
                        // is the current most recent speaking time the same as last_time?
                        // if so, then the user has been silent the whole time
                        if let Some((speaking, last_time_actual)) = self.last_time_by_user.get(&user_id) {
                            if !*speaking && last_time == *last_time_actual {
                                // user has been silent the whole time
                                self.tx_queue_silent_user.send(user_id).unwrap();
                            }
                        }
                    }
                    self.silence_list = silence_list;
                }

            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::error::TryRecvError;

    use super::*;

    #[tokio::test]
    async fn test_voice_activity() {
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (tx_silent_channel, mut rx_silent_channel) = sync::mpsc::unbounded_channel();
        let (tx_silent_user, mut rx_silent_user) = sync::mpsc::unbounded_channel();
        let voice_activity = VoiceActivity::monitor(
            rx,
            Duration::from_millis(1),
            tx_silent_channel,
            tx_silent_user,
        )
        .await;

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 starts talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: true,
        })
        .unwrap();

        assert!(!rx_silent_channel.recv().await.unwrap());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 starts talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: true,
        })
        .unwrap();
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 stops talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: false,
        })
        .unwrap();
        assert_eq!(rx_silent_user.recv().await.unwrap(), 1);
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 stops talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: false,
        })
        .unwrap();

        assert!(rx_silent_channel.recv().await.unwrap());
        assert_eq!(rx_silent_user.recv().await.unwrap(), 2);
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // close the sender, which will cause the loop to exit
        drop(tx);
        voice_activity.await.unwrap();
    }

    async fn test_voice_activity_with_interruption() {
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (tx_silent_channel, mut rx_silent_channel) = sync::mpsc::unbounded_channel();
        let (tx_silent_user, mut rx_silent_user) = sync::mpsc::unbounded_channel();
        let voice_activity = VoiceActivity::monitor(
            rx,
            Duration::from_millis(10),
            tx_silent_channel,
            tx_silent_user,
        )
        .await;

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 starts talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: true,
        })
        .unwrap();

        assert!(!rx_silent_channel.recv().await.unwrap());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 starts talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: true,
        })
        .unwrap();
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 stops talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: false,
        })
        .unwrap();
        // we should NOT have sent a silent user timeout
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // now wait a while, and expect a timeout
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(rx_silent_user.recv().await.unwrap(), 1);
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 stops talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: false,
        })
        .unwrap();

        // we should NOT have fired a timeout for user 2 yet
        assert!(rx_silent_channel.recv().await.unwrap());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // before the timeout, user 2 starts talking again
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: true,
        })
        .unwrap();

        // we should still not fire a timeout for user 2, even
        // after some delay
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // close the sender, which will cause the loop to exit
        drop(tx);
        voice_activity.await.unwrap();
    }
}
