use crate::api::api_types;
use std::collections;
use tokio::sync;
use tokio::task;

#[derive(Clone, Eq, PartialEq)]
struct RecentTokens {
    created_at: u64,
    tokens: Vec<i32>,
}

// we want the oldest token to be popped first, so
// intentionally reverse the ordering
impl Ord for RecentTokens {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .created_at
            .cmp(&self.created_at)
            .then_with(|| other.tokens.cmp(&self.tokens))
    }
}

// BinaryHeap is a max-heap, so implement the most recent
// token should have the smallest value
impl PartialOrd for RecentTokens {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        other.created_at.partial_cmp(&self.created_at)
    }
}

impl RecentTokens {
    fn from(transcribed_message: api_types::TranscribedMessage) -> Self {
        let tokens = transcribed_message
            .text_segments
            .iter()
            .map(|segment| segment.tokens)
            .flatten()
            .collect();
        Self {
            created_at: transcribed_message.timestamp,
            tokens,
        }
    }
}

/// Input:
///  - finalized text segments
/// Output:
///  - when queried, most recent 1024 tokens?
struct Transcript(collections::BinaryHeap<RecentTokens>);

/// max number of tokens to produce
const NUM_TOKENS: usize = 1024;

/// number of messages to keep
const NUM_MESSAGES: usize = 10;

impl Transcript {
    pub(crate) fn monitor(
        rxQueue: sync::mpsc::Receiver<api_types::TranscribedMessage>,
    ) -> task::JoinHandle<()> {
        let transcript = Self(collections::BinaryHeap::with_capacity(NUM_MESSAGES + 1));
        task::spawn(async move {
            while let Some(transcribed_message) = rxQueue.recv().await {
                let recent_tokens = RecentTokens::from(transcribed_message);
                transcript.0.push(recent_tokens);
                if transcript.0.len() > NUM_MESSAGES {
                    // remove the oldest message
                    transcript.0.pop();
                }
            }
        })
    }

    pub(crate) fn get_latest_tokens(&self) -> Vec<i32> {
        // returns tokens from oldest to newest
        let mut tokens = self
            .0
            .iter()
            .flat_map(|recent_tokens| recent_tokens.tokens.iter())
            .copied()
            .collect::<Vec<_>>();
        if tokens.len() > NUM_TOKENS {
            tokens.drain(0..(tokens.len() - NUM_TOKENS));
        }
        tokens
    }
}
