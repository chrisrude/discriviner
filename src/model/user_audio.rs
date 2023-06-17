use std::collections::VecDeque;

use crate::{api::api_types::TranscribedMessage, events::audio::TranscriptionRequest};

use super::{
    audio_slice::AudioSlice,
    types::{self, DiscordRtcTimestamp, UserId, WhisperToken},
};

/// A buffer of audio data for a single user.
/// This buffer is split into "slices".
///
/// A slice contains a "contiguous" block of audio data.
/// A block is is "continuous" if the audio it contains has
/// contiguous periods of silence that are less than
/// USER_SILENCE_TIMEOUT_MS.
///
/// Whenever there is a period of silence that is longer than
/// USER_SILENCE_TIMEOUT_MS, a new slice is created.
///
/// This also stores the most recent tokens we've seen from
/// this user.  This is used to help inform the decoder.
pub(crate) struct UserAudio {
    pub slices: Vec<AudioSlice>,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    /// Maximum length is TOKENS_TO_KEEP.
    pub last_tokens: std::collections::VecDeque<WhisperToken>,

    /// User ID of the user that this buffer is for.
    pub user_id: UserId,
}

impl UserAudio {
    pub fn new(user_id: UserId) -> Self {
        Self {
            slices: vec![AudioSlice::new(), AudioSlice::new()],
            last_tokens: VecDeque::with_capacity(types::TOKENS_TO_KEEP),
            user_id,
        }
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[types::DiscordAudioSample],
    ) {
        // find a slice that will take the audio, and add it.
        // create a new slice if necessary.

        let slice = if let Some(slice) = self
            .slices
            .iter_mut()
            .find(|s| s.fits_within_this_slice(rtc_timestamp))
        {
            slice
        } else {
            // if we didn't find a slice, then we need to create a new one.
            self.slices.push(AudioSlice::new());
            eprintln!("created new slice, total is now {}", self.slices.len());
            self.slices.last_mut().unwrap()
        };
        slice.add_audio(rtc_timestamp, discord_audio);
    }

    pub fn handle_user_silence(&mut self) -> impl Iterator<Item = TranscribedMessage> {
        let now = std::time::SystemTime::now();
        // finalize all slices that have a finalize timestamp
        // that is older than now.
        let mut result = Vec::new();
        for slice in self.slices.iter_mut() {
            if let Some(finalize_timestamp) = slice.finalize_timestamp() {
                if finalize_timestamp < now {
                    if let Some(message) = slice.finalize() {
                        result.push(message);
                    }
                }
            }
        }
        result.into_iter()
    }

    pub fn handle_transcription_response(
        &mut self,
        message: &TranscribedMessage,
    ) -> Option<TranscribedMessage> {
        // find the slice whose start time matches the response's start time.
        // if we find it, then we can update the slice with the response.
        // if we don't find it, then we can't do anything with the response.
        let slice_opt = self.slices.iter_mut().find(|slice| {
            if let Some((_, start_timestamp)) = slice.start_time {
                start_timestamp == message.start_timestamp
            } else {
                false
            }
        });
        if let Some(slice) = slice_opt {
            slice.handle_transcription_response(message)
        } else {
            eprintln!("couldn't find slice for response");
            None
        }
    }

    pub fn try_get_transcription_requests(&mut self) -> impl Iterator<Item = TranscriptionRequest> {
        self.slices
            .iter_mut()
            .filter_map(|s| s.make_transcription_request())
            .map(|(audio_data, start_timestamp)| TranscriptionRequest {
                audio_data,
                previous_tokens: Vec::from(self.last_tokens.clone()),
                start_timestamp,
                user_id: self.user_id,
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}