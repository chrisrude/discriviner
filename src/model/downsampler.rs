// use songbird::events::context_data::VoiceData;

// use crate::{api::api_types, model::types};

// fn resample_audio_from_discord_to_whisper(audio: VoiceData) -> Vec<api_types::WhisperAudioSample> {
//     // this takes advantage of the ratio between the two sample rates
//     // being a whole number. If this is not the case, we'll need to
//     // do some more complicated resampling.
//     assert!(types::DISCORD_SAMPLES_PER_SECOND % types::WHISPER_SAMPLES_PER_SECOND == 0);
//     const BITRATE_CONVERSION_RATIO: usize =
//         types::DISCORD_SAMPLES_PER_SECOND / types::WHISPER_SAMPLES_PER_SECOND;

//     // do the conversion, we'll take the first sample, and then
//     // simply skip over the next (BITRATE_CONVERSION_RATIO-1)
//     // samples
//     //
//     // while converting the bitrate we'll also convert the audio
//     // from stereo to mono, so we'll do everything in pairs.
//     const GROUP_SIZE: usize = BITRATE_CONVERSION_RATIO * types::AUDIO_CHANNELS;

//     let out_len = audio.len() / GROUP_SIZE;
//     let mut audio_out = vec![0.0 as api_types::WhisperAudioSample; out_len];

//     let mut audio_max: api_types::WhisperAudioSample = 0.0;

//     // todo: drop audio which is very low signal?  It has had issues transcribing well.

//     // iterate through the audio vector, taking pairs of samples and averaging them
//     // while doing so, look for max and min values so that we can normalize later
//     for (i, samples) in audio.chunks_exact(GROUP_SIZE).enumerate() {
//         // take the first two values of samples, and add them into audio_out .
//         // also, find the largest absolute value, and store it in audio_max
//         let mut val = 0.0;
//         for j in 0..types::AUDIO_CHANNELS {
//             val += samples[j] as api_types::WhisperAudioSample;
//         }
//         let abs = val.abs();
//         if abs > audio_max {
//             audio_max = abs;
//         }
//         audio_out[i] = val;
//         // don't worry about dividing by AUDIO_CHANNELS, as normalizing
//         // will take care of it, saving us divisions
//     }
//     // normalize floats to be between -1 and 1
//     for sample in audio_out.iter_mut() {
//         *sample /= audio_max;
//     }
//     audio_out
// }
