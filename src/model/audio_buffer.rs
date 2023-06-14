use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};

use tokio::{sync, task};
use tokio_util::sync::CancellationToken;

use crate::{
    events::audio::{DiscordAudioData, TranscriptionRequest, TranscriptionResponse},
    model::types::WHISPER_SAMPLES_PER_MILLISECOND,
};

use super::types::{
    self, DiscordRtcTimestamp, DiscordRtcTimestampInner, UserId, WhisperAudioSample, WhisperToken,
};

pub struct AudioBuffer {
    /// Audio data lives here.
    pub buffer: std::collections::VecDeque<WhisperAudioSample>,

    /// The timestamp of the first sample in the buffer.
    /// Taken from the RTC packet.
    /// The RTC timestamp typically uses an 8khz clock.
    /// So a 20ms buffer would have a timestamp increment
    /// of 160 from the previous buffer.
    pub head_timestamp: Option<DiscordRtcTimestamp>,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    pub last_tokens: std::collections::VecDeque<WhisperToken>, // TOKENS_TO_KEEP

    /// User ID of the user that this buffer is for.
    pub user_id: Option<UserId>,
}

impl AudioBuffer {
    fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(types::WHISPER_AUDIO_BUFFER_SIZE),
            head_timestamp: None,
            last_tokens: VecDeque::with_capacity(types::TOKENS_TO_KEEP),
            user_id: None,
        }
    }

    fn assign_to_user(&mut self, user_id: UserId) {
        self.head_timestamp = None;
        self.last_tokens.clear();
        self.user_id = Some(user_id);
    }

    /////////////////////////////////////////////////
    // adding audio to the buffer

    fn add_audio(
        &mut self,
        timestamp: DiscordRtcTimestamp,
        discord_audio: &Vec<types::DiscordAudioSample>,
    ) {
        if self.head_timestamp.is_none() {
            self.head_timestamp = Some(timestamp);
        }
        let offset = Self::rtc_timestamp_delta_to_whisper_audio_delta(
            self.head_timestamp.unwrap(),
            timestamp,
        );
        let end = offset + discord_audio.len() / types::BITRATE_CONVERSION_RATIO;
        if end > self.buffer.len() {
            // todo: drop audio instead of panicking?
            panic!("buffer overflow");
        }

        if self.buffer.len() > offset {
            // todo: handle more gracefully?  Can songbird send us
            // packets out of order?
            panic!("received out-of-order packet")
        }

        if self.buffer.len() < offset {
            let num_samples_to_insert = offset - self.buffer.len();
            eprintln!(
                "skip in audio, padding with silence for {} ms",
                num_samples_to_insert / WHISPER_SAMPLES_PER_MILLISECOND
            );
            self.buffer.extend(
                std::iter::repeat(types::WhisperAudioSample::default()).take(num_samples_to_insert),
            );
        }

        self.resample_audio_from_discord_to_whisper(discord_audio);
    }

    fn rtc_timestamp_delta_to_whisper_audio_delta(
        ts1: DiscordRtcTimestamp,
        ts2: DiscordRtcTimestamp,
    ) -> usize {
        // The RTC timestamp uses an 8khz clock.
        // todo: verify
        let delta = (ts2 - ts1).0 as usize;
        if delta as DiscordRtcTimestampInner > DiscordRtcTimestampInner::MAX / 2 {
            // if the delta is more than half the max value, then it's
            // probably the case that ts1 > ts2, which means we received
            // a packet which is before our original timestamp.  In this
            // case, just panic for now, as I'm not sure if songbird
            // will ever send us packets out of order.
            // todo: handle this gracefully?
            panic!("received packet with timestamp before start of buffer")
        }
        // we want the number of 16khz samples, so just multiply by 2.
        delta * types::WHISPER_SAMPLES_PER_SECOND / types::RTC_CLOCK_SAMPLES_PER_SECOND
    }

    fn resample_audio_from_discord_to_whisper(&mut self, audio: &[types::DiscordAudioSample]) {
        for samples in audio.chunks_exact(types::BITRATE_CONVERSION_RATIO) {
            // sum the channel data, and divide by the max value possible to
            // get a value between -1.0 and 1.0
            self.buffer.push_back(
                samples
                    .iter()
                    .map(|x| *x as types::WhisperAudioSample)
                    .sum::<types::WhisperAudioSample>()
                    / types::DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES,
            );
        }
    }

    /////////////////////////////////////////////////
    // removing audio from the buffer

    fn handle_transcription_response(&mut self, response: TranscriptionResponse) {
        let _start_timestamp = response.0.start_timestamp;
        let _segments = response.0.segments;
        let _audio_duration = response.0.audio_duration;

        // todo: finish more
    }

    /// Removes the given amount of audio from the buffer,
    /// starting with the oldest audio.  When done, will pivot
    /// the remaining audio to the start of the buffer.
    fn remove_audio_duration(&mut self, duration: Duration) {
        let samples_to_remove =
            (duration.as_millis() as usize) * types::WHISPER_SAMPLES_PER_SECOND / 1000;
        if samples_to_remove > self.buffer.len() {
            self.buffer.clear();
        } else {
            self.buffer.drain(0..samples_to_remove);
            self.buffer.make_contiguous();
        }
    }
}

/// Handles a set of audio buffers, one for each user who is
/// talking in the conversation.
pub(crate) struct AudioBufferManager {
    // these are the buffers which we've assigned to a user
    // in the conversation.  We'll keep them around until the
    // user leaves, as we may need to use them again.  When
    // discarded, they will be returned to reserve_buffers.
    active_buffers: HashMap<types::UserId, AudioBuffer>,

    // these are buffers which we've pre-allocated, but which are
    // not currently in use.  We'll keep them around so that we
    // can quickly assign them to a user when they start talking.
    reserve_buffers: VecDeque<AudioBuffer>,

    // we'll have a single task which monitors this queue for new
    // audio data and then pushes it into the appropriate buffer.
    rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,

    // this queue is used to notify the audio buffer manager that
    // a user has stopped talking.  We'll use this to know when
    // to trigger our final transcription.
    rx_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,

    // this is used to signal the audio buffer manager to shut down.
    shutdown_token: CancellationToken,

    // this is used to send transcription requests to Whisper, and
    // to receive the results.
    tx_transcription_requests: sync::mpsc::UnboundedSender<TranscriptionRequest>,
}

impl<'a> AudioBufferManager {
    pub fn monitor(
        rx_audio_data: sync::mpsc::UnboundedReceiver<DiscordAudioData>,
        rx_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,
        shutdown_token: CancellationToken,
        tx_transcription_requests: sync::mpsc::UnboundedSender<TranscriptionRequest>,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer_manager = AudioBufferManager {
            active_buffers: HashMap::new(),
            reserve_buffers: VecDeque::new(),
            rx_audio_data,
            rx_silent_user_events,
            tx_transcription_requests,
            shutdown_token,
        };
        // pre-allocate some buffers
        for _ in 0..types::EXPECTED_AUDIO_PARTICIPANTS {
            audio_buffer_manager
                .reserve_buffers
                .push_back(AudioBuffer::new());
        }
        task::spawn(async move {
            audio_buffer_manager.loop_forever().await;
        })
    }

    fn with_buffer_for_user(
        &mut self,
        user_id: types::UserId,
        mut yield_fn: impl FnMut(&mut AudioBuffer),
    ) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.active_buffers.entry(user_id) {
            let mut buffer = self
                .reserve_buffers
                .pop_front()
                .unwrap_or_else(AudioBuffer::new);
            buffer.assign_to_user(user_id);
            e.insert(buffer);
        }
        let buffer = self.active_buffers.get_mut(&user_id).unwrap();
        (yield_fn)(buffer);
    }

    async fn loop_forever(&mut self) {
        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    return;
                }
                Some(
                    DiscordAudioData {
                        user_id,
                        timestamp,
                        discord_audio,
                    }
                ) = self.rx_audio_data.recv() => {
                    self.with_buffer_for_user(user_id, |buffer| {
                        buffer.add_audio(timestamp, &discord_audio);
                    });
                }
                // todo: also, every 5 seconds timeout
                Some(_user_id) = self.rx_silent_user_events.recv() => {
                    // todo: self.handle_silent_user_event(user_id).await;
                }
            }
        }
    }
}

// Called once we have a full audio clip from a user.
//     /// This is called on an event handling thread, so do not do anything
//     /// major on it, and return asap.
//     pub fn on_audio_complete(
//         &self,
//         user_id: types::UserId,
//         audio: Arc<Vec<types::DiscordAudioSample>>,
//     ) {
//         let audio_duration_ms = ((audio.len() / types::DISCORD_AUDIO_CHANNELS)
//             / types::DISCORD_SAMPLES_PER_MILLISECOND) as u32;
//         if audio_duration_ms < MIN_AUDIO_THRESHOLD_MS {
//             // very short messages are usually just noise, ignore them
//             return;
//         }
//         // get our unixtime in ms
//         let start_time = std::time::SystemTime::now();
//         let unixsecs = start_time
//             .duration_since(std::time::UNIX_EPOCH)
//             .unwrap()
//             .as_secs();

//         // make clones of everything so that the closure can own them, if
//         let audio_copy = audio;
//         let callback_copy = self.event_callback.clone();
//         let last_transcription_copy = self.last_transcription.clone();
//         let whisper_context_copy = self.whisper_context.clone();

//         // todo: if we're running too far behind, we should drop audio in order to catch up
//         // todo: if we're always running too far behind, we should display some kind of warning
//         // todo: try quantized model?

//         task::spawn(async move {
//             let whisper_audio = resample_audio_from_discord_to_whisper(audio_copy);

//             // get the last transcription, and pass it in if:
//             // - it's from the same user
//             // - the last transcription ended less than 5 seconds ago
//             let mut last_transcription_context: Option<LastTranscriptionData> = None;
//             {
//                 let last_transcription = last_transcription_copy.lock().await;
//                 let lt = last_transcription.clone();
//                 if let Some(last_transcription) = lt {
//                     if (unixsecs - last_transcription.timestamp) < 5
//                         && last_transcription.user_id == user_id
//                     {
//                         last_transcription_context = Some(last_transcription);
//                     }
//                 }
//             }
//             let text_segments = audio_to_text(
//                 &whisper_context_copy,
//                 whisper_audio,
//                 last_transcription_context,
//             );

//             // if there's nothing in the last transcription, then just stop
//             if text_segments.is_empty() {
//                 return;
//             }

//             let end_time = std::time::SystemTime::now();
//             let processing_time_ms =
//                 end_time.duration_since(start_time).unwrap().as_millis() as u32;
//             let transcribed_message = api_types::TranscribedMessage {
//                 timestamp: unixsecs,
//                 user_id,
//                 text_segments,
//                 audio_duration_ms,
//                 processing_time_ms,
//             };

//             // this is now our last transcription
//             let last_data = LastTranscriptionData::from_transcribed_message(
//                 &whisper_context_copy,
//                 transcribed_message.clone(),
//                 end_time
//                     .duration_since(std::time::UNIX_EPOCH)
//                     .unwrap()
//                     .as_secs(),
//             );
//             {
//                 last_transcription_copy.lock().await.replace(last_data);
//             }

//             callback_copy(api_types::VoiceChannelEvent::TranscribedMessage(
//                 transcribed_message,
//             ));
//         });
//     }
// }
