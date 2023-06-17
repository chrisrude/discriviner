use std::collections::VecDeque;

use whisper_rs::WhisperToken;

use crate::events::audio::TranscriptionRequest;

use super::{
    audio_slice::AudioSlice,
    constants::TOKENS_TO_KEEP,
    types::{DiscordAudioSample, DiscordRtcTimestamp, TranscribedMessage, UserId},
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
            last_tokens: VecDeque::with_capacity(TOKENS_TO_KEEP),
            user_id,
        }
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
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
        for message in result.iter() {
            self.add_tokens(message);
        }
        result.into_iter()
    }

    fn add_tokens(&mut self, message: &TranscribedMessage) {
        for segment in message.segments.iter() {
            for token in &segment.tokens_with_probability {
                self.last_tokens.push_back(token.token_id);
                if self.last_tokens.len() > TOKENS_TO_KEEP {
                    self.last_tokens.pop_front();
                }
            }
        }
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
            let result = slice.handle_transcription_response(message);
            if result.is_some() {
                self.add_tokens(&result.as_ref().unwrap());
            }
            result
        } else {
            eprintln!("couldn't find slice for response");
            None
        }
    }

    pub fn try_get_transcription_requests(&mut self) -> impl Iterator<Item = TranscriptionRequest> {
        self.slices
            .iter_mut()
            .filter_map(|s| s.make_transcription_request())
            .map(
                |(audio_data, audio_duration, start_timestamp)| TranscriptionRequest {
                    audio_data,
                    audio_duration,
                    previous_tokens: Vec::from(self.last_tokens.clone()),
                    start_timestamp,
                    user_id: self.user_id,
                },
            )
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod tests {

    use std::num::Wrapping;

    use super::*;

    #[test]
    fn test_add_audio() {
        let mut user_audio = UserAudio::new(123);
        let timestamp = Wrapping(1234);
        let audio = vec![2; 960];
        user_audio.add_audio(timestamp, &audio);
        // non empty slices
        let non_empty = user_audio
            .slices
            .iter()
            .filter(|s| s.start_time.is_some())
            .count();
        assert_eq!(non_empty, 1);
        assert_eq!(user_audio.slices[0].audio.len(), 960 / (2 * 3));

        user_audio.add_audio(Wrapping(1001234), &audio);
        let non_empty_2 = user_audio
            .slices
            .iter()
            .filter(|s| s.start_time.is_some())
            .count();
        assert_eq!(non_empty_2, 2);
        assert_eq!(user_audio.slices[1].audio.len(), 960 / (2 * 3));
    }
}
