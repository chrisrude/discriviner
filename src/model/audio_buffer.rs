use std::collections::{HashMap, VecDeque};

use tokio::{sync, task};

use crate::events::audio::{ConversionRequest, DiscordAudioData};

use super::types::{self, DiscordRtcTimestamp, UserId, WhisperAudioSample, WhisperToken};

pub struct AudioBuffer {
    /// Audio data lives here.
    pub buffer: std::collections::VecDeque<WhisperAudioSample>,

    /// The timestamp of the first sample in the buffer.
    /// Taken from the RTC packet.
    /// The RTC timestamp typically uses an 8khz clock.
    /// So a 20ms buffer would have a timestamp increment
    /// of 160 from the previous buffer.
    pub head_timestamp: DiscordRtcTimestamp,

    /// The timestamp of the last sample in the buffer.
    pub last_write_timestamp: DiscordRtcTimestamp,

    /// The timestamp of the most recent sample we've sent
    /// to the transcription worker.
    pub last_transcription_timestamp: DiscordRtcTimestamp,

    /// User ID of the user that this buffer is for.
    pub user_id: UserId,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    pub last_tokens: std::collections::VecDeque<WhisperToken>, // TOKENS_TO_KEEP
}

impl AudioBuffer {
    fn new() -> Self {
        Self {
            user_id: 0, // TODO
            buffer: VecDeque::with_capacity(types::WHISPER_AUDIO_BUFFER_SIZE),
            head_timestamp: std::num::Wrapping(0), // todo: sentinel value?
            last_transcription_timestamp: std::num::Wrapping(0),
            last_write_timestamp: std::num::Wrapping(0),
            last_tokens: VecDeque::with_capacity(types::TOKENS_TO_KEEP),
        }
    }
}

/// Handles a set of audio buffers, one for each user who is
/// talking in the conversation.
pub(crate) struct AudioBufferManager {
    // we'll have a single task which monitors this queue for new
    // audio data and then pushes it into the appropriate buffer.
    rx_queue_voice: sync::mpsc::UnboundedReceiver<DiscordAudioData>,

    // this queue is used to notify the audio buffer manager that
    // a user has stopped talking.  We'll use this to know when
    // to trigger our final transcription.
    rx_queue_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,

    // this is used to send transcription requests to Whisper, and
    // to receive the results.
    tx_queue_conversion_requests: sync::mpsc::UnboundedSender<ConversionRequest>,

    // these are the buffers which we've assigned to a user
    // in the conversation.  We'll keep them around until the
    // user leaves, as we may need to use them again.  When
    // discarded, they will be returned to reserve_buffers.
    active_buffers: HashMap<types::UserId, AudioBuffer>,

    // these are buffers which we've pre-allocated, but which are
    // not currently in use.  We'll keep them around so that we
    // can quickly assign them to a user when they start talking.
    reserve_buffers: VecDeque<AudioBuffer>,
}

impl<'a> AudioBufferManager {
    pub fn monitor(
        rx_queue_voice: sync::mpsc::UnboundedReceiver<DiscordAudioData>,
        rx_queue_silent_user_events: sync::mpsc::UnboundedReceiver<types::UserId>,
        tx_queue_conversion_requests: sync::mpsc::UnboundedSender<ConversionRequest>,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer_manager = AudioBufferManager {
            rx_queue_voice,
            rx_queue_silent_user_events,
            tx_queue_conversion_requests,
            active_buffers: HashMap::new(),
            reserve_buffers: VecDeque::new(),
        };
        // pre-allocate some buffers
        for _ in 0..10 {
            audio_buffer_manager
                .reserve_buffers
                .push_back(AudioBuffer::new());
        }
        task::spawn(async move {
            audio_buffer_manager.loop_forever().await;
        })
    }

    async fn loop_forever(&mut self) {
        loop {
            tokio::select! {
                Some(_voice_data) = self.rx_queue_voice.recv() => {
                    // todo: self.handle_voice_data(voice_data).await;
                }
                Some(_user_id) = self.rx_queue_silent_user_events.recv() => {
                    // todo: self.handle_silent_user_event(user_id).await;
                }
            }
        }
    }
}

const DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES: types::WhisperAudioSample =
    types::DISCORD_AUDIO_MAX_VALUE * types::DISCORD_AUDIO_CHANNELS as types::WhisperAudioSample;

fn resample_audio_from_discord_to_whisper(
    audio: &[types::DiscordAudioSample; types::DISCORD_PACKET_GROUP_SIZE],
    audio_out: &mut [types::WhisperAudioSample; types::WHISPER_PACKET_GROUP_SIZE],
) {
    for (i, samples) in audio
        .chunks_exact(types::BITRATE_CONVERSION_RATIO)
        .enumerate()
    {
        // sum the channel data, and divide by the max value possible to
        // get a value between -1.0 and 1.0
        audio_out[i] = samples
            .iter()
            .map(|x| *x as types::WhisperAudioSample)
            .sum::<types::WhisperAudioSample>()
            / DISCORD_AUDIO_MAX_VALUE_TWO_SAMPLES;
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
