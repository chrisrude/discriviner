use std::{path::Path, sync::Arc, time::Duration};

use tokio::task::JoinHandle;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

use crate::{
    api::api_types,
    events::audio::{TranscriptionRequest, TranscriptionResponse},
};

use super::types::{self, WhisperAudioSample, WHISPER_SAMPLES_PER_MILLISECOND};

pub(crate) struct Whisper {
    rx_transcription_requests: tokio::sync::mpsc::UnboundedReceiver<TranscriptionRequest>,
    shutdown_token: tokio_util::sync::CancellationToken,
    whisper_context: Arc<WhisperContext>,
}

impl Whisper {
    /// Load a model from the given path
    pub fn load_and_monitor(
        model_path: String,
        rx_transcription_requests: tokio::sync::mpsc::UnboundedReceiver<TranscriptionRequest>,
        shutdown_token: tokio_util::sync::CancellationToken,
    ) -> JoinHandle<()> {
        let path = Path::new(model_path.as_str());
        if !path.exists() {
            panic!("Model file does not exist: {}", path.to_str().unwrap());
        }
        if !path.is_file() {
            panic!("Model is not a file: {}", path.to_str().unwrap());
        }

        let whisper_context =
            Arc::new(WhisperContext::new(model_path.as_str()).expect("failed to load model"));

        let mut whisper = Self {
            rx_transcription_requests,
            shutdown_token,
            whisper_context,
        };

        tokio::task::spawn(async move {
            whisper.convert_forever().await;
        })
    }

    async fn convert_forever(&mut self) {
        // select on both the shutdown token and the transcription requests
        tokio::select! {
            _ = self.shutdown_token.cancelled() => {
                // we're done
                return;
            }
            Some(transcription_request) = self.rx_transcription_requests.recv() => {
                self.process_transcription_request(transcription_request).await;

            }
        }
    }

    async fn process_transcription_request(
        &self,
        TranscriptionRequest {
            audio_data,
            previous_tokens,
            response_queue,
            start_timestamp,
            user_id,
        }: TranscriptionRequest,
    ) {
        let processing_start = std::time::Instant::now();
        let whisper_context_clone = self.whisper_context.clone();
        let conversion_task = tokio::task::spawn_blocking(move || {
            Self::audio_to_text(&whisper_context_clone, audio_data, previous_tokens)
        });

        let (text_segments, result_tokens, audio_duration) = conversion_task.await.unwrap();

        let response = TranscriptionResponse {
            tokens: result_tokens,
            message: api_types::TranscribedMessage {
                start_timestamp,
                user_id,
                text_segments,
                audio_duration,
                processing_time: processing_start.elapsed(),
            },
        };

        response_queue.send(response).unwrap();
    }

    /// This will take a long time to run, don't call it
    /// on a tokio event thread.
    /// ctx came from load_model
    /// audio data should be is f32, 16KHz, mono
    fn audio_to_text(
        whisper_context: &WhisperContext,
        audio_data_bytes: bytes::Bytes,
        previous_tokens: Vec<types::WhisperToken>,
    ) -> (
        Vec<api_types::TextSegment>,
        Vec<types::WhisperToken>,
        Duration,
    ) {
        let mut state = whisper_context.create_state().unwrap();

        let audio_len_bytes = audio_data_bytes.len();
        let audio_len_samples = audio_len_bytes / std::mem::size_of::<WhisperAudioSample>();
        let audio_data = unsafe {
            std::slice::from_raw_parts(
                audio_data_bytes.as_ptr() as *const WhisperAudioSample,
                audio_len_samples,
            )
        };
        let audio_duration =
            Duration::from_millis((audio_len_samples / WHISPER_SAMPLES_PER_MILLISECOND) as u64);

        // actually convert audio to text.  Takes a while.
        state
            .full(Self::make_params(&previous_tokens), audio_data)
            .unwrap();

        let num_segments = state.full_n_segments().unwrap();
        let mut result_segments =
            Vec::<api_types::TextSegment>::with_capacity(num_segments as usize);
        let mut result_tokens = Vec::<types::WhisperToken>::new();
        for i in 0..num_segments {
            result_segments.push(api_types::TextSegment {
                text: state.full_get_segment_text(i).unwrap().to_string(),
                start_offset_ms: state.full_get_segment_t0(i).unwrap() as u32,
                end_offset_ms: state.full_get_segment_t1(i).unwrap() as u32,
            });

            result_tokens.push(state.full_n_tokens(i).unwrap())
        }
        (result_segments, result_tokens, audio_duration)
    }

    fn make_params(previous_tokens: &Vec<types::WhisperToken>) -> FullParams<'_, '_> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        params.set_tokens(previous_tokens.as_slice());

        params
    }
}
