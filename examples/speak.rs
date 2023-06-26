// use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use std::{fs::File, io::Write, iter, mem, slice};

use rubato::{
    calculate_cutoff, Resampler, SincFixedOut, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use songbird::constants::MONO_FRAME_SIZE;

const ESPEAKNG_SAMPLE_RATE: usize = 22050;
const DISCORD_SAMPLE_RATE: usize = 48000;

const CONVERSION_RATIO: f64 = DISCORD_SAMPLE_RATE as f64 / ESPEAKNG_SAMPLE_RATE as f64;

fn main() {
    let spoken = discrivener::speak("Hello, world!");
    println!("Sample rate is {}", spoken.sample_rate);
    write_to_file(ESPEAKNG_SAMPLE_RATE, &spoken.wav);

    assert_eq!(spoken.sample_rate as usize, ESPEAKNG_SAMPLE_RATE);

    let discord_audio = resample(&spoken.wav);
    write_to_file(DISCORD_SAMPLE_RATE, &discord_audio);
}

/// number of samples needed to fully store input_frames
/// after conversion to Discord's sample rate, rounded
/// up to the nearest multiple of MONO_FRAME_SIZE
fn calc_output_frames(input_frames: usize) -> usize {
    let frames = (input_frames as f64 * CONVERSION_RATIO).ceil() as usize;

    // find the next multiple of MONO_FRAME_SIZE that is no less than frames
    let remainder = frames % MONO_FRAME_SIZE;
    if remainder == 0 {
        frames
    } else {
        frames + MONO_FRAME_SIZE - remainder
    }
}

fn write_to_file(sample_rate: usize, data: &[i16]) {
    let filename_with_sample_rate = format!("test-{}.pcm", sample_rate);

    let mut file: File = File::create(filename_with_sample_rate).unwrap();
    let slice_u8 = unsafe {
        slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() * mem::size_of::<u16>(),
        )
    };

    file.write_all(&slice_u8).unwrap();
    file.sync_all().unwrap();
}

fn init_resampler() -> SincFixedOut<f64> {
    // increase this to increase f_cutoff through weird estimate math
    let sinc_len = 128;
    let window = WindowFunction::Blackman2;
    let f_cutoff = calculate_cutoff(sinc_len, window);
    // note: audio generated at 22050hz, Nyquist frequency is 11025hz
    // so no need to go higher than that
    eprintln!("f_cutoff: {}", f_cutoff);
    eprintln!("mono frame size: {}", MONO_FRAME_SIZE);

    SincFixedOut::<f64>::new(
        CONVERSION_RATIO,
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

fn resample(data: &[i16]) -> Vec<i16> {
    let mut resampler = init_resampler();

    // make an iterator which can provide an f64 translation of
    // each i16 in data, and then pad the end with zeroes.
    // note that this will now iterate forever, so we will
    // measure the output to know when to stop.
    let mut data_f64 = data
        .iter()
        .map(|x| *x as f64 / i16::MAX as f64)
        .into_iter()
        .chain(iter::repeat(0.0));

    let out_frames = calc_output_frames(data.len());

    let mut written_frames: usize = 0;
    let waves_out = &mut [vec![0.0f64; out_frames]];
    while written_frames < out_frames {
        let in_size = Resampler::input_frames_next(&resampler);
        let wave_in = (&mut data_f64).take(in_size).collect::<Vec<f64>>();
        let wave_out = &mut waves_out[0][written_frames..written_frames + MONO_FRAME_SIZE];

        let (_, n_written) =
            Resampler::process_into_buffer(&mut resampler, &[wave_in], &mut [wave_out], None)
                .unwrap();

        written_frames += n_written;
    }

    // convert waves_out back to i16 with range -32768 to 32767
    waves_out[0]
        .iter()
        .map(|x| (x * i16::MAX as f64) as i16)
        .collect()
}
