/// Resample audio from espeak-ng to Discord's sample rate.
/// This is necessary because espeak-ng generates audio at 22050hz,
/// and Discord expects audio at 48000hz.
use std::iter;

use rubato::{
    calculate_cutoff, SincFixedOut, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use songbird::constants::MONO_FRAME_SIZE;

/// number of samples needed to fully store input_frames
/// after conversion to Discord's sample rate, rounded
/// up to the nearest multiple of MONO_FRAME_SIZE
fn calc_output_frames(input_frames: usize, resample_ratio: f64) -> usize {
    let frames = (input_frames as f64 * resample_ratio).ceil() as usize;

    // find the next multiple of MONO_FRAME_SIZE that is no less than frames
    let remainder = frames % MONO_FRAME_SIZE;
    if remainder == 0 {
        frames
    } else {
        frames + MONO_FRAME_SIZE - remainder
    }
}

fn init_resampler(resample_ratio: f64) -> SincFixedOut<f64> {
    // increase this to increase f_cutoff through weird estimate math
    let sinc_len = 128;
    let window = WindowFunction::Blackman2;
    let f_cutoff = calculate_cutoff(sinc_len, window);

    SincFixedOut::<f64>::new(
        resample_ratio,
        1.0,
        SincInterpolationParameters {
            sinc_len,
            f_cutoff,
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 512,
            window,
        },
        MONO_FRAME_SIZE,
        1,
    )
    .unwrap()
}

pub(crate) fn resample(from_sample_rate: usize, to_sample_rate: usize, data: &[i16]) -> Vec<i16> {
    let resample_ratio = to_sample_rate as f64 / from_sample_rate as f64;
    let mut resampler = init_resampler(resample_ratio);

    // make an iterator which can provide an f64 translation of
    // each i16 in data, and then pad the end with zeroes.
    // note that this will now iterate forever, so we will
    // measure the output to know when to stop.
    let mut data_f64 = data
        .iter()
        .map(|x| *x as f64 / i16::MAX as f64)
        .chain(iter::repeat(0.0));

    let out_frames = calc_output_frames(data.len(), resample_ratio);

    let mut written_frames: usize = 0;
    let waves_out = &mut [vec![0.0f64; out_frames]];
    while written_frames < out_frames {
        let in_size = rubato::Resampler::input_frames_next(&resampler);
        let wave_in = (&mut data_f64).take(in_size).collect::<Vec<f64>>();
        let wave_out = &mut waves_out[0][written_frames..written_frames + MONO_FRAME_SIZE];

        let (_, n_written) = rubato::Resampler::process_into_buffer(
            &mut resampler,
            &[wave_in],
            &mut [wave_out],
            None,
        )
        .unwrap();

        written_frames += n_written;
    }

    // convert waves_out back to i16 with range -32768 to 32767
    waves_out[0]
        .iter()
        .map(|x| (x * i16::MAX as f64) as i16)
        .collect()
}
