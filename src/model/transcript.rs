use crate::api::api_types;
use std::collections;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync;
use tokio::task;

/// Input:
///  - finalized text segments
/// Output:
///  - when queried, most recent lines of text
struct Transcript(collections::BinaryHeap<api_types::TranscribedMessage>);

impl Transcript {
    pub(crate) async fn monitor(
        mut rx_queue: sync::mpsc::UnboundedReceiver<api_types::TranscribedMessage>,
        num_messages: usize,
    ) -> (task::JoinHandle<()>, Arc<Mutex<Self>>) {
        let transcript = Arc::new(Mutex::new(Self(collections::BinaryHeap::with_capacity(
            num_messages + 1,
        ))));
        let transcript_clone = transcript.clone();
        let join_handle = task::spawn(async move {
            while let Some(transcribed_message) = rx_queue.recv().await {
                let mut transcript_locked = transcript_clone.lock().unwrap();
                transcript_locked.0.push(transcribed_message);
                if transcript_locked.0.len() > num_messages {
                    // remove the oldest message
                    transcript_locked.0.pop();
                }
            }
        });
        (join_handle, transcript)
    }

    pub(crate) async fn get_latest_text(&self) -> String {
        // returns text from oldest to newest
        self.0
            .iter()
            .map(|message| message.all_text())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) async fn get_latest_text_for_user(&self, user_id: api_types::UserId) -> String {
        // returns text from oldest to newest
        self.0
            .iter()
            .filter(|message| message.user_id == user_id)
            .map(|message| message.all_text())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_transcript() {
        const NUM_MESSAGES: usize = 3;
        let (tx_queue, rx_queue) = sync::mpsc::unbounded_channel();
        let (handle, activity_monitor) = super::Transcript::monitor(rx_queue, NUM_MESSAGES).await;
        assert_eq! {
            activity_monitor.lock().unwrap().get_latest_text().await,
            ""
        };
        assert_eq! {
            activity_monitor.lock().unwrap().get_latest_text_for_user(1).await,
            ""
        };
        drop(tx_queue);
        handle.await.unwrap();

        // tx_queue
        //     .send(api_types::TranscribedMessage {
        //         user_id: 1,
        //         text: "hello".to_string(),
        //     })
        //     .await
        //     .unwrap();
    }
}
