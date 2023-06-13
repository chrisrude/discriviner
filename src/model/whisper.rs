use std::{path::Path, sync::Arc};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

pub struct Whisper {
    whisper_context: Arc<WhisperContext>,
}

fn make_params() -> FullParams<'static, 'static> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params
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
}

// /// ctx came from load_model
// /// audio data should be is f32, 16KHz, mono
// fn audio_to_text(
//     whisper_context: &Arc<WhisperContext>,
//     audio_data: Vec<types::WhisperAudioSample>,
//     last_transcription: Option<LastTranscriptionData>,
// ) -> Vec<api_types::TextSegment> {
//     let mut state = whisper_context.create_state().unwrap();

//     let mut params = make_params();

//     // if we have a last_transcription, add it to the state
//     let last_tokens: LastTranscriptionData;
//     if let Some(last_tokens_tmp) = last_transcription {
//         // assign to a variable to ensure lifetime is long enough
//         last_tokens = last_tokens_tmp;
//         params.set_tokens(&last_tokens.tokens[..]);
//     }

//     // actually convert audio to text.  Takes a while.
//     state.full(params, &audio_data[..]).unwrap();

//     // todo: use a different context / token history for each user
//     // see https://github.com/ggerganov/whisper.cpp/blob/57543c169e27312e7546d07ed0d8c6eb806ebc36/examples/stream/stream.cpp

//     let num_segments = state.full_n_segments().unwrap();
//     let mut result = Vec::<api_types::TextSegment>::with_capacity(num_segments as usize);
//     for i in 0..num_segments {
//         result.push(api_types::TextSegment {
//             text: state.full_get_segment_text(i).unwrap().to_string(),
//             start_offset_ms: state.full_get_segment_t0(i).unwrap() as u32,
//             end_offset_ms: state.full_get_segment_t1(i).unwrap() as u32,
//         });
//     }
//     result
// }
