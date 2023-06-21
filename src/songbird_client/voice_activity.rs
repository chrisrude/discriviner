use crate::audio::events::UserAudioEvent;
use crate::audio::events::UserAudioEventType;
use crate::model::constants::FOREVER;
use crate::model::types::UserId;
use crate::model::types::VoiceChannelEvent;
use std::collections;
use std::collections::VecDeque;
use tokio::sync;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

struct SpeakingUsers {
    speaking_users: collections::HashSet<UserId>,
    tx_api_events: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
}

impl SpeakingUsers {
    pub fn new(tx_api_events: sync::mpsc::UnboundedSender<VoiceChannelEvent>) -> Self {
        Self {
            speaking_users: collections::HashSet::new(),
            tx_api_events,
        }
    }

    pub fn add(&mut self, user_id: UserId) {
        self.speaking_users.insert(user_id);
        if self.speaking_users.len() == 1 {
            self.announce(false);
        }
    }

    pub fn remove(&mut self, user_id: &UserId) {
        self.speaking_users.remove(user_id);
        if self.speaking_users.is_empty() {
            self.announce(true);
        }
    }

    fn announce(&mut self, is_silent: bool) {
        self.tx_api_events
            .send(VoiceChannelEvent::ChannelSilent(is_silent))
            .ok();
    }
}

pub(crate) struct VoiceActivity {
    last_time_by_user: collections::HashMap<UserId, (bool, time::Instant)>,
    rx_voice_activity: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
    shutdown_token: CancellationToken,
    silence_list: VecDeque<(Instant, UserId, Instant)>,
    speaking_users: SpeakingUsers,
    tx_silent_user_events: sync::mpsc::UnboundedSender<UserAudioEvent>,
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
    pub(crate) fn monitor(
        rx_voice_activity: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
        shutdown_token: CancellationToken,
        tx_api_events: sync::mpsc::UnboundedSender<VoiceChannelEvent>,
        tx_silent_user_events: sync::mpsc::UnboundedSender<UserAudioEvent>,
        user_silence_timeout: Duration,
    ) -> task::JoinHandle<()> {
        let mut voice_activity = Self {
            last_time_by_user: collections::HashMap::new(),
            rx_voice_activity,
            shutdown_token,
            silence_list: VecDeque::new(),
            speaking_users: SpeakingUsers::new(tx_api_events.clone()),
            tx_silent_user_events,
            user_silence_timeout,
        };
        task::spawn(async move {
            voice_activity.loop_forever().await;
        })
    }

    async fn loop_forever(&mut self) {
        loop {
            let forever = time::Instant::now().checked_add(FOREVER).unwrap();
            let noop_time = (forever, 0, forever);

            // next future will always be at the end... only need to wait for a single one ever
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // time to be done
                    return;
                }
                Some(activity) = self.rx_voice_activity.recv() => {
                    match activity.event_type {
                        UserAudioEventType::Speaking => {
                            self.handle_user_speaking(&activity.user_id);
                        }
                        UserAudioEventType::Silent => {
                            self.handle_user_silent(&activity.user_id);
                        }
                        _ => {}
                    };
                }
                () = time::sleep_until(
                    self.silence_list.get(0).unwrap_or(&noop_time).0
                ) => {
                    self.process_next_idle_timeout();
                }
            }
        }
    }

    /// Sends an event if the user has been silent for the
    /// self.user_silence_timeout interval.
    fn process_next_idle_timeout(&mut self) {
        let (_, user_id, last_time) = self.silence_list.pop_front().unwrap();
        // is the current most recent speaking time the same as last_time?
        // if so, then the user has been silent the whole time
        if let Some((speaking, last_time_actual)) = self.last_time_by_user.get(&user_id) {
            if !*speaking && last_time == *last_time_actual {
                // user has been silent the whole time
                println!(
                    "user {:?} has been silent for {:?}",
                    user_id, self.user_silence_timeout
                );
                self.tx_silent_user_events
                    .send(UserAudioEvent {
                        event_type: UserAudioEventType::Idle,
                        user_id,
                    })
                    .unwrap();
            }
        }
    }

    /// Handle a user speaking.  This will send a "non-silent channel"
    /// event if this is the first user speaking.
    fn handle_user_speaking(&mut self, user_id: &UserId) {
        self.speaking_users.add(*user_id);

        // mark the time the user was last heard from.  This will
        // prevent the user idle event from being triggered.
        self.last_time_by_user
            .insert(*user_id, (true, Instant::now()));
    }

    /// Handle a user becoming silent.  This will send a "silent channel"
    /// event if this was the only user speaking.  It will also set a timer
    /// to fire an "idle user" event, should it remain silent for
    /// self.user_silence_timeout.
    fn handle_user_silent(&mut self, user_id: &UserId) {
        self.speaking_users.remove(user_id);

        let now = time::Instant::now();
        // store the last time we heard from this user.  If the user
        // has not spoken again at the end of this interval, then
        // we will send a silent user event
        self.last_time_by_user.insert(*user_id, (false, now));

        self.silence_list
            .push_back((now + self.user_silence_timeout, *user_id, now));

        // forward user-silent event to the main audio listener
        self.tx_silent_user_events
            .send(UserAudioEvent {
                user_id: *user_id,
                event_type: UserAudioEventType::Silent,
            })
            .unwrap();
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
            shutdown_token.clone(),
            tx_silent_channel,
            tx_silent_user,
            Duration::from_millis(1),
        );

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 1 starts talking
        tx.send(UserAudioEvent {
            user_id: 1,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();

        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 starts talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 1 stops talking
        tx.send(UserAudioEvent {
            user_id: 1,
            event_type: UserAudioEventType::Silent,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Silent)));
        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Idle)));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 stops talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Silent,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Silent)));
        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(silent);
        } else {
            panic!("expected silent channel event");
        }
        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Idle)));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // close the sender, which will cause the loop to exit
        shutdown_token.cancel();
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
            shutdown_token.clone(),
            tx_silent_channel,
            tx_silent_user,
            Duration::from_millis(10),
        );

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 1 starts talking
        tx.send(UserAudioEvent {
            user_id: 1,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();

        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 starts talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 1 stops talking
        tx.send(UserAudioEvent {
            user_id: 1,
            event_type: UserAudioEventType::Silent,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;

        // we should NOT have sent a silent user timeout
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Silent)));
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // now wait a while, and expect a timeout
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Idle)));
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 stops talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Silent,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(rx_silent_user
            .try_recv()
            .is_ok_and(|x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Silent)));

        // we should NOT have fired a timeout for user 2 yet
        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // before the timeout, user 2 starts talking again
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Speaking,
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
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        shutdown_token.cancel();
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
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        println!("sending shutdown token 2");
        shutdown_token.cancel();
        voice_activity.await.unwrap();
    }
}
