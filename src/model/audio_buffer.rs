use std::collections::VecDeque;

use crate::model::types;
use bytes::Bytes;
use tokio::{sync, task};

use super::types::DiscordVoiceData;

struct AudioBuffer {
    buffer: VecDeque<types::WhisperAudioSample>,
    write_head: usize,
    last_write_timestamp: u32,
    rx_queue: sync::mpsc::Receiver<types::DiscordVoiceData>,
}

impl AudioBuffer {
    pub fn monitor(
        rx_queue: sync::mpsc::Receiver<types::DiscordVoiceData>,
    ) -> task::JoinHandle<()> {
        let mut audio_buffer = AudioBuffer {
            buffer: VecDeque::with_capacity(types::WHISPER_AUDIO_BUFFER_SIZE),
            last_write_timestamp: 0,
            rx_queue,
            write_head: 0,
        };
        task::spawn(async move {
            audio_buffer.loop_forever().await;
        })
    }

    async fn loop_forever(&mut self) {
        while let Some(audio_data) = self.rx_queue.recv().await {
            let mem = Bytes::from(self.buffer);
            self.push(&audio_data);
        }
    }

    fn push(&mut self, audio_data: &DiscordVoiceData) {
        // convert to whisper audio sample
        self.buffer.push_back(types::WhisperAudioSample {
            timestamp: audio_data.timestamp,
            data: Bytes::copy_from_slice(&audio_data.data),
        });
    }
}

// type AudioBuffer = ringbuf::HeapRb<types::AudioSample>;

// pub(crate) struct VoiceBuffer {
//     // store 30 seconds of audio, 16-bit stereo PCM at 48kHz
//     // divided into 20ms chunks

//     // whenever we fill up a buffer, we'll send it to decoding.
//     // we have A & B buffers, so that one can be filled while the other is being
//     // decoded.
//     buffer_mutex: Arc<Mutex<AudioBuffer>>,

//     receiver: sync::mpsc::Receiver<Vec<types::MyVoiceData>>,
// }

// impl VoiceBuffer {
//     pub(crate) fn new(callback: types::AudioCallback) -> Self {
//         let buffer = AudioBuffer::new(types::AUDIO_BUFFER_SIZE);

//         Self {
//             buffer_mutex: Arc::new(Mutex::new(buffer)),
//             on_buffer_full_fn: callback,
//         }
//     }

//     /// If the current buffer is full, flush it and return the other buffer.
//     /// Flushing means calling the callback with the current buffer, which
//     /// should consume everything in the buffer.
//     /// In any case, returns the buffer that we should be writing to.
//     fn push(&self, audio: &Vec<types::AudioSample>) {
//         // if we have enough space in the current buffer, push it there.
//         // if not, mark the buffer as full and put all the audio in the
//         // other buffer.
//         let m = self.buffer_mutex.clone();
//         let mut buffer = m.lock().unwrap();
//         let (mut producer, consumer) = buffer.split_ref();

//         if producer.free_len() < audio.len() {}

//         producer.push_slice(audio.as_slice());
//     }
// }

// pub struct VoiceBufferForUser {
//     pub user_id: types::UserId,
//     buffer: VoiceBuffer,
//     speaking: bool,
// }

// impl VoiceBufferForUser {
//     pub fn new(user_id: types::UserId, callback: types::AudioCallback) -> Self {
//         Self {
//             user_id,
//             buffer: VoiceBuffer::new(callback),
//             speaking: true,
//         }
//     }

//     pub fn push(&self, audio: &Vec<types::AudioSample>) {
//         if !self.speaking {
//             if audio.iter().all(|sample| *sample == 0) {
//                 return;
//             }
//             eprintln!("got audio for non-speaking user {}", self.user_id);
//             return;
//         }
//         self.buffer.push(audio);
//     }

//     /// Called when a user has started talking after a period of silence.
//     /// This is NOT called when a user starts talking for the first time.
//     pub fn on_start_talking(&mut self) {
//         self.speaking = true;
//     }

//     pub fn on_stop_talking(&mut self) {
//         self.speaking = false;
//     }
// }
