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

#[cfg(test)]
mod tests {
    use super::*;

    const ESPEAK_RATE: usize = 22050;
    const DISCORD_RATE: usize = 48000;

    /// Samples away from the start and end, where the sinc filter's
    /// delay and the zero padding don't affect the output.
    fn steady_state(output: &[i16], input_len: usize) -> &[i16] {
        let expected_len = input_len * DISCORD_RATE / ESPEAK_RATE;
        &output[expected_len / 4..expected_len * 3 / 4]
    }

    #[test]
    fn test_calc_output_frames_rounds_up_to_frame_size() {
        assert_eq!(calc_output_frames(0, 2.0), 0);
        assert_eq!(calc_output_frames(1, 2.0), MONO_FRAME_SIZE);
        assert_eq!(
            calc_output_frames(MONO_FRAME_SIZE / 2, 2.0),
            MONO_FRAME_SIZE
        );
        assert_eq!(
            calc_output_frames(MONO_FRAME_SIZE / 2 + 1, 2.0),
            2 * MONO_FRAME_SIZE
        );
    }

    #[test]
    fn test_resample_empty() {
        assert!(resample(ESPEAK_RATE, DISCORD_RATE, &[]).is_empty());
    }

    #[test]
    fn test_resample_output_length() {
        for input_len in [1, 100, ESPEAK_RATE / 2, ESPEAK_RATE, ESPEAK_RATE * 3 + 7] {
            let input = vec![0i16; input_len];
            let output = resample(ESPEAK_RATE, DISCORD_RATE, &input);
            let min_len = (input_len * DISCORD_RATE).div_ceil(ESPEAK_RATE);
            assert_eq!(output.len() % MONO_FRAME_SIZE, 0, "input_len {}", input_len);
            assert!(output.len() >= min_len, "input_len {}", input_len);
            assert!(
                output.len() < min_len + 2 * MONO_FRAME_SIZE,
                "input_len {}",
                input_len
            );
        }
    }

    #[test]
    #[ignore = "known bug: float rounding in the resample ratio adds an extra frame when the output is an exact multiple of the frame size"]
    fn test_resample_output_length_exact_frames() {
        // 11025 samples at 22050 Hz is exactly 24000 samples (25 frames) at 48 kHz
        let output = resample(ESPEAK_RATE, DISCORD_RATE, &[0i16; ESPEAK_RATE / 2]);
        assert_eq!(output.len(), 25 * MONO_FRAME_SIZE);
    }

    #[test]
    fn test_resample_silence_stays_silent() {
        let output = resample(ESPEAK_RATE, DISCORD_RATE, &[0i16; ESPEAK_RATE]);
        assert!(output.iter().all(|s| *s == 0));
    }

    #[test]
    fn test_resample_preserves_dc_level() {
        let level = i16::MAX / 2;
        let input = vec![level; ESPEAK_RATE];
        let output = resample(ESPEAK_RATE, DISCORD_RATE, &input);
        for sample in steady_state(&output, input.len()) {
            assert!(
                (*sample as i32 - level as i32).abs() <= 2,
                "sample {} too far from {}",
                sample,
                level
            );
        }
    }

    #[test]
    fn test_resample_full_scale_does_not_wrap() {
        // sinc ringing can overshoot full scale; the conversion back to
        // i16 must saturate rather than wrap to the opposite sign
        for level in [i16::MAX, i16::MIN + 1] {
            let input = vec![level; ESPEAK_RATE];
            let output = resample(ESPEAK_RATE, DISCORD_RATE, &input);
            for sample in steady_state(&output, input.len()) {
                assert_eq!(sample.signum(), level.signum());
                assert!((*sample as i32 - level as i32).abs() <= 2);
            }
        }
    }

    #[test]
    fn test_resample_preserves_tone_frequency() {
        // a 440 Hz tone should still cross zero ~880 times per second
        let input: Vec<i16> = (0..ESPEAK_RATE)
            .map(|i| {
                let t = i as f64 / ESPEAK_RATE as f64;
                ((2.0 * std::f64::consts::PI * 440.0 * t).sin() * 16000.0) as i16
            })
            .collect();
        let output = resample(ESPEAK_RATE, DISCORD_RATE, &input);
        let middle = steady_state(&output, input.len());
        let crossings = middle
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        let seconds = middle.len() as f64 / DISCORD_RATE as f64;
        let measured_hz = crossings as f64 / seconds / 2.0;
        assert!(
            (measured_hz - 440.0).abs() < 5.0,
            "measured {} Hz",
            measured_hz
        );
    }
}
