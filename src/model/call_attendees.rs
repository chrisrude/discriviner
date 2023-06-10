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
struct CallAttendees(collections::HashMap<types::Ssrc, types::UserId>);

impl CallAttendees {
    pub(crate) fn monitor(
        rxQueue: sync::mpsc::Receiver<(types::Ssrc, api_types::UserJoinData)>,
    ) -> task::JoinHandle<()> {
        let attendees = Self(collections::HashMap::new());
        task::spawn(async move {
            while let Some((ssrc, api_types::UserJoinData { user_id, joined })) =
                rxQueue.recv().await
            {
                if joined {
                    attendees.0.insert(ssrc, user_id);
                }
                // don't remove users from the map when they leave, because
                // we may still be processing their audio.  Instead, if another
                // user joins with the same SSRC, we'll overwrite the old user.
                // There's a chance here that a user could leave and then another
                // rejoins with the same SSRC, but if that happened it's the
                // server that's broken, not us.
            }
        })
    }

    pub(crate) fn get_user(&self, ssrc: types::Ssrc) -> Option<types::UserId> {
        self.0.get(&ssrc).copied()
    }
}
