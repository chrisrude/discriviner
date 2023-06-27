use std::{
    io,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use songbird::input::{reader::MediaSource, Reader};
use tokio::{sync::mpsc::UnboundedReceiver, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::model::constants::DISCORD_SAMPLES_PER_SECOND;

pub(crate) struct Speaker {
    driver: Arc<Mutex<songbird::Driver>>,
    rx: UnboundedReceiver<String>,
    shutdown_token: CancellationToken,
}

impl Speaker {
    pub(crate) fn monitor(
        driver: Arc<Mutex<songbird::Driver>>,
        rx: UnboundedReceiver<String>,
        shutdown_token: CancellationToken,
    ) -> JoinHandle<()> {
        let speaker = Self {
            driver,
            rx,
            shutdown_token,
        };
        tokio::spawn(async move { speaker.run_forever() })
    }

    fn run_forever(mut self) {
        let espeakng_sample_rate = crate::audio::espeakng::init() as usize;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = self.shutdown_token.cancelled() => {
                        break;
                    }
                    Some(message) = self.rx.recv() => {
                        eprintln!("Received message: {}", message);
                        let reader = speak_to_reader(&message, DISCORD_SAMPLES_PER_SECOND, espeakng_sample_rate).await;
                        let input = songbird::input::Input::new(
                            false,
                            reader,
                            songbird::input::Codec::Pcm,
                            songbird::input::Container::Raw,
                            Default::default(),
                        );
                        eprintln!("Sending to driver");
                        self.driver.lock().unwrap().play_only_source(input);
                        eprintln!("Sent to driver");
                    }
                }
            }
        });
    }
}

/// Renders the given text as speech, encoded as mono
/// i16 PCM at the given sample rate.
async fn speak_to_reader(
    message: &str,
    discord_sample_rate: usize,
    espeakng_sample_rate: usize,
) -> Reader {
    let audio = crate::audio::espeakng::speak(message).await;

    let output_buffer = if espeakng_sample_rate == discord_sample_rate {
        audio
    } else {
        crate::audio::resample::resample(
            espeakng_sample_rate,
            discord_sample_rate,
            audio.as_slice(),
        )
    };

    Reader::Extension(Box::new(VecMediaSource::new(output_buffer)))
}

pub(crate) struct VecMediaSource {
    data: Bytes,
    pos: usize,
}

impl VecMediaSource {
    pub fn new(data: Vec<i16>) -> Self {
        let data_slice = data.as_slice();

        // convert to bytes
        let byte_data = unsafe {
            std::slice::from_raw_parts(
                data_slice.as_ptr() as *const u8,
                std::mem::size_of_val(data_slice),
            )
        };

        VecMediaSource {
            data: Bytes::from(byte_data),
            pos: 0,
        }
    }
}

impl io::Read for VecMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_to_read = buf.len().min(self.data.len() - self.pos);
        // copy data into buf
        buf[..bytes_to_read].copy_from_slice(&self.data[self.pos..self.pos + bytes_to_read]);
        self.pos += bytes_to_read;
        Ok(bytes_to_read)
    }
}

impl io::Seek for VecMediaSource {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.pos = match pos {
            io::SeekFrom::Start(pos) => pos as usize,
            io::SeekFrom::End(pos) => (self.data.len() as i64 + pos) as usize,
            io::SeekFrom::Current(pos) => (self.pos as i64 + pos) as usize,
        };
        self.pos = self.pos.min(self.data.len());
        Ok(self.pos as u64)
    }
}

impl MediaSource for VecMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        let len = self.data.len();
        if len > 0 {
            Some(len as u64)
        } else {
            None
        }
    }
}
