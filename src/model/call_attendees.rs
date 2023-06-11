use crate::api::api_types;
use crate::model::types;
use std::collections;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync;
use tokio::task;

/// Input:
///  - user_joined event
///  - user_left event
/// Output:
///  - stores map of SSID -> user
struct CallAttendees(collections::HashMap<types::Ssrc, api_types::UserId>);

impl CallAttendees {
    pub(crate) async fn monitor(
        mut rx_queue: sync::mpsc::Receiver<(types::Ssrc, api_types::UserJoinData)>,
    ) -> (task::JoinHandle<()>, Arc<Mutex<Self>>) {
        let attendees = Arc::new(Mutex::new(Self(collections::HashMap::new())));
        let attendees_clone = attendees.clone();
        let join_handle = task::spawn(async move {
            while let Some((ssrc, api_types::UserJoinData { user_id, joined })) =
                rx_queue.recv().await
            {
                if joined {
                    attendees.lock().unwrap().0.insert(ssrc, user_id);
                } else {
                    attendees.lock().unwrap().0.remove(&ssrc);
                }
            }
        });
        (join_handle, attendees_clone)
    }

    pub(crate) async fn get_user_id_from_ssrc(
        &self,
        ssrc: types::Ssrc,
    ) -> Option<api_types::UserId> {
        self.0.get(&ssrc).copied()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::api::api_types;
    use crate::model::types;
    use tokio::sync;

    #[tokio::test]
    async fn test_call_attendees() {
        let (tx, mut rx) = sync::mpsc::channel(1);
        let (join_handle, attendees) = CallAttendees::monitor(rx).await;
        tx.send((
            1,
            api_types::UserJoinData {
                user_id: 1,
                joined: true,
            },
        ))
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(1).await,
            Some(1)
        );
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(2).await,
            None
        );

        tx.send((
            2,
            api_types::UserJoinData {
                user_id: 2,
                joined: true,
            },
        ))
        .await
        .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(1).await,
            Some(1)
        );
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(2).await,
            Some(2)
        );

        tx.send((
            1,
            api_types::UserJoinData {
                user_id: 3,
                joined: true,
            },
        ))
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(1).await,
            Some(3)
        );
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(2).await,
            Some(2)
        );

        tx.send((
            1,
            api_types::UserJoinData {
                user_id: 3,
                joined: false,
            },
        ))
        .await
        .unwrap();
        tx.send((
            2,
            api_types::UserJoinData {
                user_id: 2,
                joined: false,
            },
        ))
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(1).await,
            None
        );
        assert_eq!(
            attendees.lock().unwrap().get_user_id_from_ssrc(2).await,
            None
        );

        drop(tx);
        join_handle.await.unwrap();
    }
}
