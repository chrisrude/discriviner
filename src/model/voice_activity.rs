use crate::events::audio::VoiceActivityData;
use crate::model::types::UserId;
use std::collections;
use std::collections::VecDeque;
use tokio::sync;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::types::VoiceChannelEvent;

pub(crate) struct VoiceActivity {
    last_time_by_user: collections::HashMap<UserId, (bool, time::Instant)>,
    rx_voice_activity: sync::mpsc::UnboundedReceiver<VoiceActivityData>,
    shutdown_token: CancellationToken,
    silence_list: VecDeque<(Instant, UserId, Instant)>,
    speaking_users: collections::HashSet<UserId>,
    tx_api_events: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
    tx_silent_user_events: sync::mpsc::UnboundedSender<(UserId, bool)>,
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

    pub(crate) fn monitor(
        rx_voice_activity: sync::mpsc::UnboundedReceiver<VoiceActivityData>,
        shutdown_token: CancellationToken,
        tx_api_events: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        tx_silent_user_events: sync::mpsc::UnboundedSender<(UserId, bool)>,
        user_silence_timeout: Duration,
    ) -> task::JoinHandle<()> {
        let mut voice_activity = Self {
            last_time_by_user: collections::HashMap::new(),
            rx_voice_activity,
            shutdown_token,
            silence_list: VecDeque::new(),
            speaking_users: collections::HashSet::new(),
            tx_api_events,
            tx_silent_user_events,
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
                _ = self.shutdown_token.cancelled() => {
                    // time to be done
                    return;
                }
                maybe_activity = self.rx_voice_activity.recv() => {
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
                            self.tx_api_events.send(
                                VoiceChannelEvent::ChannelSilent(false)
                            ).unwrap();
                        }
                    } else {
                        let was_someone_speaking = !self.speaking_users.is_empty();
                        self.speaking_users.remove(&activity.user_id);
                        if was_someone_speaking && self.speaking_users.is_empty() {
                            self.tx_api_events.send(
                                VoiceChannelEvent::ChannelSilent(true)
                            ).unwrap();
                        }
                    }

                    // forward on silent user events.  These will be used
                    // to eagerly kick off transcriptions.
                    // todo: use the general event channel instead?
                    if !activity.speaking {
                        self.tx_silent_user_events.send((activity.user_id, false)).unwrap();
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
                                self.tx_silent_user_events.send((user_id, true)).unwrap();
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
        let shutdown_token = CancellationToken::new();
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (tx_silent_channel, mut rx_silent_channel) = sync::mpsc::unbounded_channel();
        let (tx_silent_user, mut rx_silent_user) = sync::mpsc::unbounded_channel();
        let voice_activity = VoiceActivity::monitor(
            rx,
            shutdown_token,
            tx_silent_channel,
            tx_silent_user,
            Duration::from_millis(1),
        );

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 starts talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: true,
        })
        .unwrap();

        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
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

        tokio::time::sleep(Duration::from_millis(2)).await;

        assert_eq!(Ok((1, false)), rx_silent_user.try_recv());
        assert_eq!(rx_silent_user.recv().await.unwrap(), (1, true));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 stops talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: false,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;

        assert_eq!(Ok((2, false)), rx_silent_user.try_recv());
        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(rx_silent_user.recv().await.unwrap(), (2, true));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // close the sender, which will cause the loop to exit
        drop(tx);
        voice_activity.await.unwrap();
    }

    #[tokio::test]
    async fn test_voice_activity_with_interruption() {
        let shutdown_token = CancellationToken::new();
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (tx_silent_channel, mut rx_silent_channel) = sync::mpsc::unbounded_channel();
        let (tx_silent_user, mut rx_silent_user) = sync::mpsc::unbounded_channel();
        let voice_activity = VoiceActivity::monitor(
            rx,
            shutdown_token,
            tx_silent_channel,
            tx_silent_user,
            Duration::from_millis(10),
        );

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 1 starts talking
        tx.send(VoiceActivityData {
            user_id: 1,
            speaking: true,
        })
        .unwrap();

        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
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

        tokio::time::sleep(Duration::from_millis(2)).await;

        // we should NOT have sent a silent user timeout
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Ok((1, false)), rx_silent_user.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // now wait a while, and expect a timeout
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(rx_silent_user.recv().await.unwrap(), (1, true));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // user 2 stops talking
        tx.send(VoiceActivityData {
            user_id: 2,
            speaking: false,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;
        assert_eq!(Ok((2, false)), rx_silent_user.try_recv());

        // we should NOT have fired a timeout for user 2 yet
        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(silent);
        } else {
            panic!("expected silent channel event");
        }
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
        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        // close the sender, which will cause the loop to exit
        drop(tx);
        voice_activity.await.unwrap();
    }

    #[tokio::test]
    async fn test_shutdown_on_token() {
        let shutdown_token = CancellationToken::new();
        let (_tx, rx) = sync::mpsc::unbounded_channel();
        let (tx_silent_channel, mut rx_silent_channel) = sync::mpsc::unbounded_channel();
        let (tx_silent_user, mut rx_silent_user) = sync::mpsc::unbounded_channel();
        let voice_activity = VoiceActivity::monitor(
            rx,
            shutdown_token.clone(),
            tx_silent_channel,
            tx_silent_user,
            Duration::from_millis(10),
        );

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert_eq!(Err(TryRecvError::Empty), rx_silent_user.try_recv());

        shutdown_token.cancel();
        voice_activity.await.unwrap();
    }
}
