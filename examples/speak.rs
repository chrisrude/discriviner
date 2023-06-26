// use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use std::{fs::File, io::Write, mem, slice};

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

const ESPEAKNG_SAMPLE_RATE: usize = 22050;
const DISCORD_SAMPLE_RATE: usize = 48000;

fn main() {
    let spoken = discrivener::speak("Hello, world!");
    println!("Sample rate is {}", spoken.sample_rate);
    assert_eq!(spoken.sample_rate as usize, ESPEAKNG_SAMPLE_RATE);

    // write_to_file(ESPEAKNG_SAMPLE_RATE, &spoken.wav);

    // resample to DISCORD_SAMPLE_RATE

    let discord_audio = resample(&spoken.wav);
    write_to_file(DISCORD_SAMPLE_RATE, &discord_audio);
}

fn write_to_file(sample_rate: usize, data: &[i16]) {
    let filename_with_sample_rate = format!("test-{}.pcm", sample_rate);

    let mut file: File = File::create(filename_with_sample_rate).unwrap();
    let slice_u8 = unsafe {
        slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() / mem::size_of::<u16>(),
        )
    };

    file.write_all(&slice_u8).unwrap();
    file.sync_all().unwrap();
}

fn resample(data: &[i16]) -> Vec<i16> {
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    // convert i16 to f64 with range -1.0 to 1.0
    let data_f64: Vec<f64> = data.iter().map(|x| *x as f64 / i16::MAX as f64).collect();

    assert_eq!(data_f64.len(), data.len());

    let mut resampler = SincFixedIn::<f64>::new(
        DISCORD_SAMPLE_RATE as f64 / ESPEAKNG_SAMPLE_RATE as f64,
        1.0,
        params,
        1024,
        1,
    )
    .unwrap();

    // todo: work through entire buffer...

    let waves_in = vec![data_f64; 1];
    let out_buffer = resampler.output_buffer_allocate(false);

    let waves_out = resampler.process(&waves_in, None).unwrap();

    println!("{} {}", waves_out[0].len(), waves_in[0].len());
    assert!(waves_out[0].len() > waves_in[0].len());

    // convert waves_out[0] back to i16 with range -32768 to 32767
    let i16_out: Vec<i16> = waves_out[0]
        .iter()
        .map(|x| (x * i16::MAX as f64) as i16)
        .collect();
    assert_eq!(waves_out[0].len(), i16_out.len());

    return i16_out;
}
