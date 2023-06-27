#![allow(non_upper_case_globals)]

use espeakng_sys::*;
use lazy_static::lazy_static;
use songbird::constants::MONO_FRAME_SIZE;
use std::ffi::{c_short, c_void, CString};
use std::os::raw::c_int;
use std::sync::Mutex;
use tokio::sync::oneshot;

use crate::model::constants::{DISCORD_SAMPLES_PER_SECOND, ESPEAK_SAMPLES_PER_SECOND};

const BUFSIZE_MS: u64 = (1000 * MONO_FRAME_SIZE / DISCORD_SAMPLES_PER_SECOND) as u64;
// const EE_OK: i32 = 0;
const EE_INTERNAL_ERROR: i32 = -1;
const VOICE_NAME: &str = "English";

struct GenerationTask {
    /// The audio data, so far
    pub wav: Vec<i16>,
    /// Wake us up when the audio is ready
    pub tx: oneshot::Sender<Vec<i16>>,
}

lazy_static! {
    static ref in_process_task_opt: Mutex<Option<GenerationTask>> = Mutex::new(None);
}
pub fn init() -> i32 {
    unsafe {
        let sample_rate = espeak_Initialize(
            // send data to our callback function, which will
            // notify us when done.  We do this so that
            // we don't block this thread while waiting for
            // espeak-ng to finish.
            espeak_AUDIO_OUTPUT_AUDIO_OUTPUT_RETRIEVAL,
            // ms, not bytes!
            BUFSIZE_MS as c_int,
            // default directory for espeak-ng-data.  null
            std::ptr::null(),
            // dont exit(1) on error.  who tf would do that as a shard lib?
            espeakINITIALIZE_DONT_EXIT as c_int,
        );
        assert_ne!(EE_INTERNAL_ERROR, sample_rate);
        assert_eq!(ESPEAK_SAMPLES_PER_SECOND as i32, sample_rate);

        let cstr_voice = CString::new(VOICE_NAME).unwrap();
        let result = espeak_SetVoiceByName(cstr_voice.as_ptr());

        // this should be zero?
        assert_eq!(2, result);
        // assert_eq!(EE_OK, result);

        espeak_SetSynthCallback(Some(synth_callback));

        // return our sample rate
        sample_rate
    }
}

unsafe extern "C" fn synth_callback(
    wav: *mut c_short,
    sample_count: c_int,
    _events: *mut espeak_EVENT,
) -> c_int {
    {
        let mut task_opt = in_process_task_opt.lock().unwrap();
        // it seems the callback can happen after we finish receiving
        // audio, so if it's none just make sure we have no audio, then exit
        if task_opt.is_none() {
            if !wav.is_null() {
                eprintln!("synth_callback: wav is not null, but task is none");
            }
            return 0;
        }

        // when wav is zero, we are done
        if wav.is_null() {
            // we're done.  Remove the task from the mutex
            // and send the audio over the response channel
            let task = task_opt.take().unwrap();
            task.tx.send(task.wav).unwrap();
        } else {
            let new_audio = std::slice::from_raw_parts(wav, sample_count as usize);
            // add new_audio to the task's wav
            task_opt
                .as_mut()
                .map(|task| task.wav.extend_from_slice(new_audio));
        }
    }

    0
}

/// Perform Text-To-Speech
pub async fn speak(text: &str) -> Vec<i16> {
    let (tx, rx) = oneshot::channel::<Vec<i16>>();

    // there best not be another transcription in progress!
    {
        let mut opt_task = in_process_task_opt.lock().unwrap();

        // if this fails, it means we either didn't clean up an earlier task,
        // or more than one is running at the same time.
        assert!(opt_task.is_none());

        let new_task = GenerationTask {
            wav: Vec::new(),
            tx,
        };
        *opt_task = Some(new_task);
    }

    let cstr_text = CString::new(text).unwrap();

    unsafe {
        let str_bytes = cstr_text.as_bytes_with_nul();
        espeak_Synth(
            str_bytes.as_ptr() as *const c_void,
            str_bytes.len() as u64,
            0,
            espeak_POSITION_TYPE_POS_CHARACTER,
            0,
            espeakCHARS_AUTO,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    };

    rx.await.unwrap()
}
