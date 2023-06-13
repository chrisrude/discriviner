use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

use tokio::sync;

use crate::events::audio::{AudioBuffer, AudioBufferForUser, DiscordVoiceData};

use super::types;

// this takes advantage of the ratio between the two sample rates
// being a whole number. If this is not the case, we'll need to
// do some more complicated resampling.
const BITRATE_CONVERSION_RATIO: usize =
    types::DISCORD_SAMPLES_PER_SECOND / types::WHISPER_SAMPLES_PER_SECOND;

// do the conversion, we'll take the first sample, and then
// simply skip over the next (BITRATE_CONVERSION_RATIO-1)
// samples
//
// while converting the bitrate we'll also convert the audio
// from stereo to mono, so we'll do everything in pairs.
const GROUP_RATIO: usize = BITRATE_CONVERSION_RATIO * types::DISCORD_AUDIO_CHANNELS;

impl AudioBuffer {
    fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(types::WHISPER_AUDIO_BUFFER_SIZE),
            head_timestamp: 0,
            last_transcription_timestamp: 0,
            last_write_timestamp: 0,
        }
    }
}

impl AudioBufferForUser {
    fn new(user_id: types::UserId) -> Self {
        Self {
            user_id,
            buffers: [AudioBuffer::new(), AudioBuffer::new()],
            current_write_buffer: Mutex::new(0),
            last_tokens: VecDeque::with_capacity(types::TOKENS_TO_KEEP),
            worker_task: Mutex::new(None),
        }
    }
}

/// Handles a set of audio buffers, one for each user who is
/// talking in the conversation.
struct AudioBufferManager {
    // we'll have a single task which monitors this queue for new
    // audio data and then pushes it into the appropriate buffer.
    rx_queue: sync::mpsc::Receiver<DiscordVoiceData>,

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

// impl AudioBufferManager {
//     pub fn monitor(rx_queue: sync::mpsc::Receiver<DiscordVoiceData>) -> task::JoinHandle<()> {
//         let audio_buffer_manager = AudioBufferManager {
//             rx_queue,
//             active_buffers: HashMap::new(),
//             reserve_buffers: VecDeque::new(),
//         };
//         // pre-allocate some buffers
//         for _ in 0..10 {
//             audio_buffer_manager
//                 .reserve_buffers
//                 .push_back(AudioBuffer::new());
//         }
//         task::spawn(async move {
//             audio_buffer_manager.loop_forever().await;
//         })
//     }

//     async fn loop_forever(&mut self) {
//         while let Some(audio_data) = self.rx_queue.recv().await {
//             // get a slice to store data into.
//             // todo: all the things
//         }
//     }
// }

// fn resample_audio_from_discord_to_whisper(
//     audio: Arc<Vec<types::DiscordAudioSample>>,
//     audio_out: &[types::WhisperAudioSample; types::WHISPER_AUDIO_BUFFER_SIZE],
// ) {
//     assert!(audio.len() % GROUP_RATIO == 0);
//     assert!(audio_out.len() == audio.len() / GROUP_RATIO);

//     let audio_max: types::WhisperAudioSample = types::DiscordAudioSample::MAX
//         as types::WhisperAudioSample
//         * types::DISCORD_AUDIO_CHANNELS as types::WhisperAudioSample;

//     for (i, samples) in audio.chunks_exact(GROUP_RATIO).enumerate() {
//         // sum the channel data, and divide by the max value possible to
//         // get a value between -1.0 and 1.0
//         audio_out[i] = samples
//             .iter()
//             .map(|x| *x as types::WhisperAudioSample)
//             .sum::<types::WhisperAudioSample>()
//             / audio_max;
//     }
// }

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
