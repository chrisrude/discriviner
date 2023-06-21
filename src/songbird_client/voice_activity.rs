use crate::audio::events::UserAudioEvent;
use crate::audio::events::UserAudioEventType;
use crate::model::constants::FOREVER;
use crate::model::types::UserId;
use crate::model::types::VoiceChannelEvent;
use std::collections;
use std::collections::BinaryHeap;
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

    pub fn add(&mut self, user_id: &UserId) {
        self.speaking_users.insert(*user_id);
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

#[derive(Eq, Ord, PartialEq)]
struct UserTime {
    user_id: UserId,
    idle_timeout: time::Instant,
}

impl PartialOrd for UserTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.idle_timeout.partial_cmp(&other.idle_timeout)
    }
}

struct UserIdleDetector {
    idle_times: BinaryHeap<UserTime>,
    tx_silent_user_events: sync::mpsc::UnboundedSender<UserAudioEvent>,
    user_silence_timeout: Duration,
}

impl UserIdleDetector {
    fn new(
        tx_silent_user_events: sync::mpsc::UnboundedSender<UserAudioEvent>,
        user_silence_timeout: Duration,
    ) -> Self {
        Self {
            idle_times: BinaryHeap::new(),
            tx_silent_user_events,
            user_silence_timeout,
        }
    }

    pub fn on_speaking(&mut self, user_id: &UserId) {
        self.purge_user(user_id);
    }

    pub fn on_silent(&mut self, user_id: &UserId) {
        self.purge_user(user_id);
        self.idle_times.push(UserTime {
            user_id: *user_id,
            idle_timeout: time::Instant::now() + self.user_silence_timeout,
        });
    }

    pub fn on_idle_timeout(&mut self) {
        if let Some(UserTime {
            user_id,
            idle_timeout,
        }) = self.idle_times.pop()
        {
            assert!(idle_timeout <= Instant::now());
            self.tx_silent_user_events
                .send(UserAudioEvent {
                    user_id,
                    event_type: UserAudioEventType::Idle,
                })
                .unwrap();
        }
    }

    pub fn next_timeout(&self) -> Instant {
        // return the next soonest idle timeout
        self.idle_times
            .peek()
            .map(|user_time| user_time.idle_timeout)
            .unwrap_or(Instant::now() + FOREVER)
    }

    fn purge_user(&mut self, user_id: &UserId) {
        self.idle_times
            .retain(|user_time| user_time.user_id != *user_id);
    }
}

pub(crate) struct VoiceActivity {
    rx_voice_activity: sync::mpsc::UnboundedReceiver<UserAudioEvent>,
    shutdown_token: CancellationToken,
    speaking_users: SpeakingUsers,
    tx_silent_user_events: sync::mpsc::UnboundedSender<UserAudioEvent>,
    user_idle_detector: UserIdleDetector,
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
        let tx_silent_user_events_clone = tx_silent_user_events.clone();
        let mut voice_activity = Self {
            rx_voice_activity,
            shutdown_token,
            speaking_users: SpeakingUsers::new(tx_api_events.clone()),
            tx_silent_user_events,
            user_idle_detector: UserIdleDetector::new(
                tx_silent_user_events_clone,
                user_silence_timeout,
            ),
        };
        task::spawn(async move {
            voice_activity.loop_forever().await;
        })
    }

    async fn loop_forever(&mut self) {
        let idle_timeout = time::sleep(FOREVER);
        tokio::pin!(idle_timeout);

        loop {
            idle_timeout
                .as_mut()
                .reset(self.user_idle_detector.next_timeout());

            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    // time to be done
                    return;
                }
                _ = idle_timeout.as_mut() => {
                    self.user_idle_detector.on_idle_timeout()
                }
                Some(event) = self.rx_voice_activity.recv() => {
                    let UserAudioEvent { user_id, event_type } = &event;
                    match event_type {
                        UserAudioEventType::Speaking => {
                            self.speaking_users.add(&user_id);
                            self.user_idle_detector.on_speaking(&user_id);
                        }
                        UserAudioEventType::Silent => {
                            self.speaking_users.remove(&user_id);
                            self.user_idle_detector.on_silent(&user_id);
                        }
                        _ => {}
                    };
                    // forward silent events to the main audio listener
                    self.tx_silent_user_events.send(event).unwrap();
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

        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user.try_recv().is_ok_and(
            |x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Speaking)
        ));
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 starts talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user.try_recv().is_ok_and(
            |x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Speaking)
        ));
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

        tokio::time::sleep(Duration::from_millis(10)).await;

        if let VoiceChannelEvent::ChannelSilent(silent) = rx_silent_channel.recv().await.unwrap() {
            assert!(!silent);
        } else {
            panic!("expected silent channel event");
        }
        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user.try_recv().is_ok_and(
            |x| (x.user_id == 1) && matches!(x.event_type, UserAudioEventType::Speaking)
        ));
        assert!(rx_silent_user
            .try_recv()
            .is_err_and(|x| TryRecvError::Empty.eq(&x)));

        // user 2 starts talking
        tx.send(UserAudioEvent {
            user_id: 2,
            event_type: UserAudioEventType::Speaking,
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(Err(TryRecvError::Empty), rx_silent_channel.try_recv());
        assert!(rx_silent_user.try_recv().is_ok_and(
            |x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Speaking)
        ));
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

        assert!(rx_silent_user.try_recv().is_ok_and(
            |x| (x.user_id == 2) && matches!(x.event_type, UserAudioEventType::Speaking)
        ));
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
