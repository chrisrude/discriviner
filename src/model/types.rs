use std::{
    num::Wrapping,
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use songbird::events::context_data;

pub(crate) type DiscordAudioSample = i16;
pub(crate) type DiscordRtcTimestampInner = u32;
pub(crate) type DiscordRtcTimestamp = Wrapping<DiscordRtcTimestampInner>;
pub(crate) type Ssrc = u32;
pub(crate) type UserId = u64;
pub(crate) type WhisperAudioSample = f32;

// all this is because the songbird types don't implement Serialize
// and Deserialize, and we want to use that to print these structures
// as JSON

#[serde_as]
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
pub struct Transcription {
    /// absolute time this message was received,
    /// as reported by the Discord server
    /// (NOT the local machine time)
    pub start_timestamp: SystemTime,

    /// Discord user id of the speaker
    pub user_id: u64,

    /// One or more text segments extracted
    /// from the audio.
    pub segments: Vec<TextSegment>,

    /// conversion metric: total time of source
    /// audio which lead to this message
    pub audio_duration: Duration,

    /// total time spent converting this audio
    /// to text
    pub processing_time: Duration,
}

#[serde_as]
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Serialize, Deserialize)]
pub struct TokenWithProbability {
    pub probability: u32,
    pub token_id: i32,
    pub token_text: String,
}

#[serde_as]
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Serialize, Deserialize)]
pub struct TextSegment {
    /// When the audio for this segment started.
    /// Time is relative to when the Message was received.
    pub start_offset_ms: u32,

    /// When the audio for this segment ended.
    /// Time is relative to when the Message was received.
    pub end_offset_ms: u32,

    pub tokens_with_probability: Vec<TokenWithProbability>,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize)]
pub struct ConnectData {
    /// ID of the voice channel being joined, if it is known.
    ///
    /// If this is available, then this can be used to reconnect/renew
    /// a voice session via the gateway.
    pub channel_id: Option<u64>,
    /// ID of the target voice channel's parent guild.
    pub guild_id: u64,
    /// Unique string describing this session for validation/authentication purposes.
    pub session_id: String,
    /// The domain name of Discord's voice/TURN server.
    ///
    /// With the introduction of Discord's automatic voice server selection,
    /// this is no longer guaranteed to match a server's settings. This field
    /// may be useful if you need/wish to move your voice connection to a node/shard
    /// closer to Discord.
    pub server: String,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize)]
pub enum DisconnectKind {
    /// The voice driver failed to connect to the server.
    ///
    /// This requires explicit handling at the gateway level
    /// to either reconnect or fully disconnect.
    Connect,
    /// The voice driver failed to reconnect to the server.
    ///
    /// This requires explicit handling at the gateway level
    /// to either reconnect or fully disconnect.
    Reconnect,
    /// The voice connection was terminated mid-session by either
    /// the user or Discord.
    ///
    /// If `reason == None`, then this disconnection is either
    /// a full disconnect or a user-requested channel change.
    /// Otherwise, this is likely a session expiry (requiring user
    /// handling to fully disconnect/reconnect).
    Runtime,
    /// Some value we didn't expect.
    Unknown,
}

/// The reason that a voice connection failed.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize)]
pub enum DisconnectReason {
    /// This (re)connection attempt was dropped due to another request.
    AttemptDiscarded,
    /// Songbird had an internal error.
    ///
    /// This should never happen; if this is ever seen, raise an issue with logs.
    Internal,
    /// A host-specific I/O error caused the fault; this is likely transient, and
    /// should be retried some time later.
    Io,
    /// Songbird and Discord disagreed on the protocol used to establish a
    /// voice connection.
    ///
    /// This should never happen; if this is ever seen, raise an issue with logs.
    ProtocolViolation,
    /// A voice connection was not established in the specified time.
    TimedOut,
    /// The Websocket connection was closed by Discord.
    ///
    /// This typically indicates that the voice session has expired,
    /// and a new one needs to be requested via the gateway.
    WsClosed(Option<u32>),
    // Unexpected value
    Unknown,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize)]
pub struct DisconnectData {
    /// The location that a voice connection was terminated.
    pub kind: DisconnectKind,
    /// The cause of any connection failure.
    ///
    /// If `None`, then this disconnect was requested by the user in some way
    /// (i.e., leaving or changing voice channels).
    pub reason: Option<DisconnectReason>,
    /// ID of the voice channel being joined, if it is known.
    ///
    /// If this is available, then this can be used to reconnect/renew
    /// a voice session via thew gateway.
    pub channel_id: Option<u64>,
    /// ID of the target voice channel's parent guild.
    pub guild_id: u64,
    /// Unique string describing this session for validation/authentication purposes.
    pub session_id: String,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
pub enum VoiceChannelEvent {
    UserJoin(UserId),
    UserLeave(UserId),
    Connect(ConnectData),
    Reconnect(ConnectData),
    Disconnect(DisconnectData),
    Transcription(Transcription),
    ChannelSilent(bool),
}

impl From<context_data::DisconnectKind> for DisconnectKind {
    fn from(value: context_data::DisconnectKind) -> DisconnectKind {
        match value {
            context_data::DisconnectKind::Connect => DisconnectKind::Connect,
            context_data::DisconnectKind::Reconnect => DisconnectKind::Reconnect,
            context_data::DisconnectKind::Runtime => DisconnectKind::Runtime,
            _ => DisconnectKind::Unknown,
        }
    }
}

impl From<context_data::DisconnectReason> for DisconnectReason {
    fn from(value: context_data::DisconnectReason) -> DisconnectReason {
        match value {
            context_data::DisconnectReason::AttemptDiscarded => DisconnectReason::AttemptDiscarded,
            context_data::DisconnectReason::Internal => DisconnectReason::Internal,
            context_data::DisconnectReason::Io => DisconnectReason::Io,
            context_data::DisconnectReason::ProtocolViolation => {
                DisconnectReason::ProtocolViolation
            }
            context_data::DisconnectReason::TimedOut => DisconnectReason::TimedOut,
            context_data::DisconnectReason::WsClosed(code) => {
                DisconnectReason::WsClosed(code.map(|c| c as u32))
            }
            _ => DisconnectReason::Unknown,
        }
    }
}

impl From<&context_data::ConnectData<'_>> for ConnectData {
    fn from(value: &context_data::ConnectData<'_>) -> Self {
        ConnectData {
            channel_id: value.channel_id.map(|c| c.0),
            guild_id: value.guild_id.0,
            session_id: value.session_id.to_string(),
            server: value.server.to_string(),
        }
    }
}

impl From<&context_data::DisconnectData<'_>> for DisconnectData {
    fn from(value: &context_data::DisconnectData<'_>) -> Self {
        DisconnectData {
            kind: DisconnectKind::from(value.kind),
            reason: value.reason.map(DisconnectReason::from),
            channel_id: value.channel_id.map(|c| c.0),
            guild_id: value.guild_id.0,
            session_id: value.session_id.to_string(),
        }
    }
}

impl TextSegment {
    pub fn text(&self) -> String {
        // take all token_text values and concatenate them
        // returning the string
        let mut text = String::new();
        for token in &self.tokens_with_probability {
            text.push_str(token.token_text.as_str());
        }
        text
    }
}

impl Transcription {
    pub(crate) fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Splits the Transcription into two separate messages.
    /// The first message will contain all segments that end before the given end_time.
    /// The second message will contain all segments that end at or after the given end_time.
    ///
    /// If there are no segments that end before the given end_time, the first message
    /// will return true for .is_empty().  Same for the second message.
    ///
    /// If there are no segments in the provided message at all, both messages
    /// will be .is_empty().
    pub(crate) fn split_at_end_time(message: &Transcription, end_time: SystemTime) -> (Self, Self) {
        let fn_first_half = |segment: &TextSegment| {
            message.start_timestamp + Duration::from_millis(segment.end_offset_ms as u64)
                <= end_time
        };
        let mut first_segments = vec![];
        let mut second_segments = vec![];
        for segment in message.segments.iter() {
            if fn_first_half(segment) {
                first_segments.push(segment.clone());
            } else {
                second_segments.push(segment.clone());
            }
        }

        // eprintln!("first_segments: {:?}", first_segments);

        let first_duration = first_segments
            .iter()
            .map(|segment| Duration::from_millis(segment.end_offset_ms as u64))
            .max()
            .unwrap_or(Duration::ZERO);

        let first_transcript = Self {
            segments: first_segments,
            start_timestamp: message.start_timestamp,
            user_id: message.user_id,
            audio_duration: first_duration,
            processing_time: message.processing_time,
        };

        let second_duration = message.audio_duration - first_duration;

        // for all the second segments, remove the amount of time chopped
        // off the beginning of the message
        for segment in &mut second_segments {
            segment.start_offset_ms -= first_duration.as_millis() as u32;
            segment.end_offset_ms -= first_duration.as_millis() as u32;
        }

        let second_transcript = Self {
            segments: second_segments,
            start_timestamp: message.start_timestamp + first_duration,
            user_id: message.user_id,
            audio_duration: second_duration,
            processing_time: Duration::from_millis(1),
        };

        (first_transcript, second_transcript)
    }

    pub(crate) fn text(&self) -> String {
        // take all token_text values and concatenate them
        // returning the string
        let mut text = String::new();
        for segment in &self.segments {
            text.push_str(segment.text().as_str());
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_split_at_end_time() {
        let message = Transcription {
            segments: vec![
                TextSegment {
                    tokens_with_probability: vec![TokenWithProbability {
                        token_id: 0,
                        token_text: "hello".to_string(),
                        probability: 50,
                    }],
                    start_offset_ms: 0,
                    end_offset_ms: 1000,
                },
                TextSegment {
                    tokens_with_probability: vec![TokenWithProbability {
                        token_id: 1,
                        token_text: "world".to_string(),
                        probability: 50,
                    }],
                    start_offset_ms: 1000,
                    end_offset_ms: 2000,
                },
            ],
            start_timestamp: SystemTime::UNIX_EPOCH,
            user_id: 0,
            audio_duration: Duration::from_secs(2),
            processing_time: Duration::from_millis(1),
        };
        let (first, second) = Transcription::split_at_end_time(
            &message,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        );
        let first_segments = first.segments;
        let second_segments = second.segments;
        assert_eq!(first_segments.len(), 1);
        assert_eq!(second_segments.len(), 1);
        assert_eq!(
            first_segments[0].tokens_with_probability[0].token_text,
            "hello"
        );
        assert_eq!(
            second_segments[0].tokens_with_probability[0].token_text,
            "world"
        );
        assert_eq!(
            second.start_timestamp,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert_eq!(second_segments[0].start_offset_ms, 0)
    }
}
