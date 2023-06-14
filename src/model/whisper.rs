use std::path::Path;

use tokio::task::JoinHandle;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

use crate::{
    api::api_types,
    events::audio::{ConversionRequest, ConversionResponse},
};

use super::types;

pub struct Whisper<'a> {
    whisper_context: WhisperContext,
    rx_queue: tokio::sync::mpsc::UnboundedReceiver<ConversionRequest<'a>>,
}

impl<'a> Whisper<'a> {
    /// Load a model from the given path
    pub fn load_and_monitor(
        model_path: String,
        rx_queue: tokio::sync::mpsc::UnboundedReceiver<ConversionRequest>,
    ) -> JoinHandle<()> {
        let path = Path::new(model_path.as_str());
        if !path.exists() {
            panic!("Model file does not exist: {}", path.to_str().unwrap());
        }
        if !path.is_file() {
            panic!("Model is not a file: {}", path.to_str().unwrap());
        }

        let whisper_context =
            WhisperContext::new(model_path.as_str()).expect("failed to load model");

        let whisper = Self {
            rx_queue,
            whisper_context,
        };

        tokio::task::spawn_blocking(move || {
            async { whisper.convert_forever().await };
            ()
        })
    }

    async fn convert_forever(&mut self) {
        while let Some(ConversionRequest {
            audio_data,
            previous_tokens,
            response_queue,
            timestamp_start,
            user_id,
        }) = self.rx_queue.recv().await
        {
            let (result_segments, result_tokens) = self.audio_to_text(audio_data, previous_tokens);

            let num_segments = result_segments.len();

            let response = ConversionResponse {
                tokens: result_tokens,
                message: api_types::TranscribedMessage {
                    timestamp: 0, // TODO: SET
                    user_id: 0,   // TODO: SET
                    text_segments: result_segments,
                    audio_duration_ms: 0,  // TODO: SET
                    processing_time_ms: 0, // TODO: SET
                },
                finalized_segment_count: num_segments - 1,
            };

            response_queue.send(response).unwrap();
        }
    }

    /// This will take a long time to run, don't call it
    /// on a tokio event thread.
    /// ctx came from load_model
    /// audio data should be is f32, 16KHz, mono
    fn audio_to_text(
        &self,
        audio_data: &[types::WhisperAudioSample],
        previous_tokens: Option<&[types::WhisperToken]>,
    ) -> (Vec<api_types::TextSegment>, Vec<types::WhisperToken>) {
        let mut state = self.whisper_context.create_state().unwrap();

        // actually convert audio to text.  Takes a while.
        state
            .full(self.make_params(previous_tokens), &audio_data[..])
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
        (result_segments, result_tokens)
    }

    fn make_params(&self, previous_tokens: Option<&[types::WhisperToken]>) -> FullParams {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        if let Some(tokens) = previous_tokens {
            params.set_tokens(tokens);
        }

        params
    }
}
