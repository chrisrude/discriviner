// use crate::model::types;
// use ringbuf::{Consumer, Producer};
// use std::sync::Arc;
// use std::sync::Mutex;
// use tokio::sync;

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

// impl<'a> VoiceBuffer {
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
