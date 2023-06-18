use std::collections::VecDeque;

use whisper_rs::WhisperToken;

use crate::events::audio::TranscriptionRequest;

use super::{
    audio_slice::AudioSlice,
    constants::TOKENS_TO_KEEP,
    types::{DiscordAudioSample, DiscordRtcTimestamp, Transcription, UserId},
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
    pub slice: AudioSlice,

    /// The most recent tokens we've seen from this
    /// user.  Used to help inform the decoder.
    /// Maximum length is TOKENS_TO_KEEP.
    pub last_tokens: std::collections::VecDeque<WhisperToken>,

    /// User ID of the user that this buffer is for.
    pub user_id: UserId,

    /// If true, the user is not speaking at this instant.
    /// (though they may had been speaking an instant ago)
    pub user_silent: bool,
}

impl UserAudio {
    pub fn new(user_id: UserId) -> Self {
        Self {
            // todo: , AudioSlice::new(2)
            slice: AudioSlice::new(user_id),
            last_tokens: VecDeque::with_capacity(TOKENS_TO_KEEP),
            user_id,
            user_silent: false,
        }
    }

    pub fn add_audio(
        &mut self,
        rtc_timestamp: DiscordRtcTimestamp,
        discord_audio: &[DiscordAudioSample],
    ) {
        // find a slice that will take the audio, and add it.
        // create a new slice if necessary.
        self.user_silent = false;
        self.slice.add_audio(rtc_timestamp, discord_audio);
    }

    pub fn handle_user_idle(&mut self) -> Option<Transcription> {
        // assert!(self.user_silent);
        if !self.user_silent {
            eprintln!("user is not silent, user inactive called anyway!");
            // todo: fix!
        }
        self.slice.finalize()
    }

    pub fn set_silent(&mut self) {
        assert!(!self.user_silent);
        self.user_silent = true;
    }

    fn add_tokens(&mut self, message: &Transcription) {
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
        message: &Transcription,
    ) -> Option<Transcription> {
        let result = self.slice.handle_transcription_response(message);
        if result.is_some() {
            self.add_tokens(result.as_ref().unwrap());
        }
        result
    }

    pub fn try_get_transcription_request(&mut self) -> Option<TranscriptionRequest> {
        if let Some((audio_data, audio_duration, start_timestamp)) =
            self.slice.make_transcription_request(self.user_silent)
        {
            Some(TranscriptionRequest {
                audio_data,
                audio_duration,
                previous_tokens: Vec::from(self.last_tokens.clone()),
                start_timestamp,
                user_id: self.user_id,
            })
        } else {
            None
        }
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
        assert!(user_audio.slice.start_time.is_some());
        assert_eq!(user_audio.slice.audio.len(), 960 / (2 * 3));

        user_audio.add_audio(Wrapping(1001234), &audio);
        assert_eq!(user_audio.slice.audio.len(), 960 / (2 * 3));
    }
}
