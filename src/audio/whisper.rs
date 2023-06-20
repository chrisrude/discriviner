use std::{path::Path, sync::Arc};

use bytes::Bytes;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperToken};

use crate::{
    audio::events::{TranscriptionRequest, TranscriptionResponse},
    model::types::{TextSegment, TokenWithProbability, Transcription, WhisperAudioSample},
};

pub(crate) struct Whisper {
    whisper_context: Arc<WhisperContext>,
}

impl Whisper {
    /// Load a model from the given path
    pub fn load(model_path: String) -> Self {
        let path = Path::new(model_path.as_str());
        if !path.exists() {
            panic!("Model file does not exist: {}", path.to_str().unwrap());
        }
        if !path.is_file() {
            panic!("Model is not a file: {}", path.to_str().unwrap());
        }

        let whisper_context =
            Arc::new(WhisperContext::new(model_path.as_str()).expect("failed to load model"));

        Self { whisper_context }
    }

    pub(crate) async fn process_transcription_request(
        &self,
        TranscriptionRequest {
            audio_bytes,
            audio_duration,
            previous_tokens,
            start_timestamp,
            user_id,
        }: TranscriptionRequest,
    ) -> TranscriptionResponse {
        let processing_start = std::time::Instant::now();
        let whisper_context_clone = self.whisper_context.clone();
        let conversion_task = tokio::task::spawn_blocking(move || {
            Self::audio_to_text(&whisper_context_clone, audio_bytes, previous_tokens)
        });

        let segments = conversion_task.await.unwrap();

        let transcript = Transcription {
            start_timestamp,
            user_id,
            segments,
            audio_duration,
            processing_time: processing_start.elapsed(),
        };

        TranscriptionResponse { transcript }
    }

    /// This will take a long time to run, don't call it
    /// on a tokio event thread.
    /// ctx came from load_model
    /// audio data should be is f32, 16KHz, mono
    fn audio_to_text(
        whisper_context: &WhisperContext,
        audio_bytes: Bytes,
        previous_tokens: Vec<WhisperToken>,
    ) -> Vec<TextSegment> {
        let audio_len_bytes = audio_bytes.len();
        let audio_len_samples = audio_len_bytes / std::mem::size_of::<WhisperAudioSample>();
        let audio_data = unsafe {
            std::slice::from_raw_parts(
                audio_bytes.as_ptr() as *const WhisperAudioSample,
                audio_len_samples,
            )
        };

        let mut state = whisper_context.create_state().unwrap();

        // actually convert audio to text.  Takes a while.
        state
            .full(Self::make_params(&previous_tokens), audio_data)
            .unwrap();

        let num_segments = state.full_n_segments().unwrap();
        let mut segments = Vec::<TextSegment>::with_capacity(num_segments as usize);
        for i in 0..num_segments {
            let num_tokens = state.full_n_tokens(i).unwrap();
            let mut tokens_with_probability =
                Vec::<TokenWithProbability>::with_capacity(num_tokens as usize);
            for j in 0..num_tokens {
                let token_text = state.full_get_token_text(i, j).unwrap();
                let token_id = state.full_get_token_id(i, j).unwrap();
                let probability = (state.full_get_token_prob(i, j).unwrap() * 100.0) as u32;

                tokens_with_probability.push(TokenWithProbability {
                    p: probability,
                    token_id,
                    token_text: token_text.to_string(),
                });
            }
            let start_offset_ms = 10 * state.full_get_segment_t0(i).unwrap() as u32;
            let end_offset_ms = 10 * state.full_get_segment_t1(i).unwrap() as u32;
            eprintln!(
                "Segment {} is {}ms to {}ms",
                i, start_offset_ms, end_offset_ms
            );
            segments.push(TextSegment {
                start_offset_ms,
                end_offset_ms,
                tokens_with_probability,
            });
        }
        segments
    }

    fn make_params(previous_tokens: &Vec<WhisperToken>) -> FullParams<'_, '_> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        params.set_tokens(previous_tokens.as_slice());
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);

        params
    }
}