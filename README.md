# discrivener

A Discord audio bot that transcribes incoming audio using Whisper.cpp

## Early-bird testing

### Download `discrivener-json` binary
Currently there are binaries for 64-bit x86 Linux (kernel 3.2+, glibc 2.17+) as well as 64-bit Intel macOS (10.7+, Lion+).  You can [download them here](https://github.com/chrisrude/discriviner/releases/tag/v0.0.1).

After downloading, be sure to mark them as executable: `chmod a+x discrivener-json-*`

### Download and extract espeak-ng-data.tar.gz

[From the same page](https://github.com/chrisrude/discriviner/releases/tag/v0.0.1), download `espeak-ng-data.tar.gz` and extract it under `/usr/local/share`.  You should wind up with a `/usr/local/share/espeak-ng-data` with a bunch of `*_data` files in it.

### Download model

Download a model of your choice from https://ggml.ggerganov.com/.
I recommend `ggml-model-whisper-base.en-q5_1.bin` as a starting point.

### Configure `oobabot`

First, upgrade `oobabot` to version 0.2.0 (or later).

If you're using the command-line `oobabot`, you'll want to regenerate your `config.yml` with

```bash
mv config.yml config.yml.orig && oobabot -c config.yml.orig --gen > config.yml
```

If you're using the `oobabot-plugin`, just upgrade to version 0.2.0 of the `oobabot-plugin`, then switch to the Advanced tab.


Now, in your `config.yml`, under `discord:`, there should be two new keys:

`discrivener_location` -- set this to the full path and filename of the `discrivener-json-*` file you downloaded
`discrivener_model_location` -- set this to the full path and filename of the `ggml-model-*.bin` file you downloaded

Restart `oobabot`, or else restart the oobabooga service (needed to show the new 'Audio' tab).

Your bot should restart and if everything works, you should see something like:


```
2023-07-07 03:40:50,320  INFO Discrivener found at ...some path.../discrivener-json
2023-07-07 03:40:50,320  INFO Discrivener model found at ...some path.../ggml-base.en.bin
```

And then later:
```
2023-07-07 03:40:53,224 DEBUG Registering audio commands
2023-07-07 03:40:53,773  INFO Registered command: lobotomize: Erase (bot)'s memory of any message before now in this channel.
2023-07-07 03:40:53,773  INFO Registered command: say: Force (bot) to say the provided message.
2023-07-07 03:40:53,774  INFO Registered command: join_voice: Have (bot) join the voice channel you are in right now.
2023-07-07 03:40:53,774  INFO Registered command: leave_voice: Have (bot) leave the voice channel it is in.
```

If this is your first time, it may take 5 to 10 minutes for Discord to recognize the two new commands: `/join_voice` and `/leave_voice`.

Then, to test:

- join a discord voice channel with your user account.  This currently must be a voice channel in a Discord server that the bot has access to
- under that same account, send the bot the command `/join_voice`.  It doesn't matter what channel you use to send this command from.

The bot should look for the voice channel you're currently in and join it.

The bot will then stay in that channel until it receives a `/leave_voice` command, or until the bot is stopped.

Talk to the bot!

If you're using the `oobabooga-plugin` (recommended), you'll now see audio logged to the "Audio" tab under a few second of latency.

If you are the only other person in the channel, the bot will respond to every message it hears.

If there is more than one human in the channel, the bot will only respond if it receives a wakeword, and then with a very low percentage chance afterwards.  This is still a work in progress.



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
