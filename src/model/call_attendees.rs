use crate::api::api_types;
use crate::model::types;
use std::collections;
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
        mut rxQueue: sync::mpsc::Receiver<(types::Ssrc, api_types::UserJoinData)>,
    ) -> task::JoinHandle<()> {
        let mut attendees = Self(collections::HashMap::new());
        task::spawn(async move {
            while let Some((ssrc, api_types::UserJoinData { user_id, joined })) =
                rxQueue.recv().await
            {
                if joined {
                    attendees.0.insert(ssrc, user_id);
                } else {
                    attendees.0.remove(&ssrc);
                }
            }
        })
    }

    pub(crate) async fn get_user_id(&self, ssrc: types::Ssrc) -> Option<api_types::UserId> {
        self.0.get(&ssrc).copied()
    }
}
