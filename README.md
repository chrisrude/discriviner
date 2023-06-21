# discrivener

A Discord audio bot that transcribes incoming audio using Whisper.cpp

## structure

```none
 [songbird]
   | connects to discord via websocket
   |    and receives UDP audio packets
   |
   + driver (us) connect -> API event
   |
   + user joins                 -> []
   +   + user starts talking    -> []
   +   | user sends voice data  -> [new_stereo_audio event 🎶]
   +   + user stops talking     -> []
   + user leaves                -> []
   |
   + driver disconnect   -> API event

## audio path

### `audio_downsampler`

Input: new_stereo_audio 🎶
Output: new_mono_audio 🎵

Behavior:
 - downsamples stereo i16 48khz down to mono f32 16hkz

### `audio_buffer`

Input:
 - new_mono_audio 🎵
 - request to purge N seconds of audio for ssid

Output: audio_buffer_updated event: 🎵

Behavior:
 - maintains a pool of audio ring buffers
 - when a ssid has data, allocates a buffer if none is present
 - when a ssid has data, adds data to buffer
 - triggers audio_buffer_updated event
 - logs warning when buffer is full, overwrites oldest data

### `text_decoder`

Input:
 - audio_buffer_updated event
 - queries for most recent lines of transcript

Output:
 - finalized text segments
 - removes audio from buffer

Behavior:
 - calls whisperer to transcribe audio to text when necessary
 - necessary means:
  - there is audio in the buffer
  - a transcription isn't already in progress
 - decides:
  - after a transcription, removes audio for all "finalized" segments
  - a segment is finalized if:
    - there is a segment after it
    - the segment end is more than 2 seconds old, and
      no newer audio has been received in that time
 - stores:
  - last 1024 decoded tokens for active buffer

==== Event types

new_stereo_audio
 - [i16] audio data
 - [u32] ssid

new_mono_audio
 - [f32] audio data
 - [u32] ssid


## Models

### `voice_activity_monitor`

Input:
 - ssid_starts_talking event
 - ssid_stops_talking event

Output:
 - answers queries about:
   - is_anyone_talking?
   - when was our most recent audio from SSID?


### `call_attendance`

Input:
 - user_joined event
 - user_left event

Output:
 - stores map of SSID -> user


### `transcript`

Input:
 - finalized text segments

Output:
 - answers queries about:
   - what is the most recent transcript?
 - fires API event for new transcript


## API events

### monitors for:
 - user joined
 - user left
 - ssid starts talking
 - ssid stops talking
 -- combine with call_attendance / voice_activity_monitor?


## Data modifiers

### `whisperer`

Input:
 - audio buffer
 - most recent 1024 tokens
 - recent transcript

Output:
 - transcription, list of segments
 - how long it took to run

Behavior:
 - serializes requests, drops old ones?


## unprocessed
 - API event re: is which user is talking
