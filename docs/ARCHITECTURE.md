# Architecture

How `discrivener` is put together: what it talks to, how audio flows through
it, and the types and timing constants that shape its behavior. For build
setup see `CONTRIBUTING.md`; for known problems and planned work see
`TODO.md` and `VOICE-UPDATES.md`.

## Context

`discrivener` is a Rust library plus two example binaries that join a Discord
voice channel, transcribe what each user says with whisper.cpp, and can speak
text back into the channel with espeak-ng. It is consumed as a **subprocess**
by the Python project `oobabot`. The library does not talk to the Discord
gateway itself: the caller obtains the voice connection details (endpoint,
session id, voice token) some other way and passes them in.

### Process interface

`discrivener-json` (`examples/discrivener-json.rs`) is the binary oobabot
runs. Its contract:

- **stdout**: one JSON object per line, one per `VoiceChannelEvent`.
- **stdin**: one line per message to speak aloud in the channel.
- **stderr**: free-form debug logging (`eprintln!` everywhere; there is no
  `log`/`tracing` integration).
- Exits on Ctrl-C.

`discrivener-cli` (`examples/discrivener-cli.rs`) is a human-readable variant
of the same thing and does not support speaking.

## Source layout

| Path | Contents |
|------|----------|
| `src/lib.rs` | module declarations; `Discrivener` (`load`, `connect`, `disconnect`, `speak`) wires every task together |
| `src/model/types.rs` | public types, including `VoiceChannelEvent` (the JSON wire format) |
| `src/model/constants.rs` | buffer sizes and timeouts |
| `src/songbird_client/` | songbird event handlers (`packet_handler.rs`) and the speaking/silent/idle state machine (`voice_activity.rs`) |
| `src/scrivening/` | per-user routing (`manager.rs`) and the per-user transcription loop (`worker.rs`) |
| `src/strategies/` | `TranscriptStrategy` trait and implementations; only `five_second_strategy.rs` is used, `default_strategy.rs` is dead code |
| `src/audio/` | audio buffer, whisper wrapper, espeak-ng FFI, resampler, speaker task, internal event types |
| `examples/` | `discrivener-json` and `discrivener-cli` |
| `tests/test.json` | captured songbird events, not yet used by any test |

## Data flow

Everything is tokio tasks connected by unbounded mpsc channels and shut down
by a single shared `CancellationToken`. `src/lib.rs` (`Discrivener::load`)
wires it together; read that function first.

```
songbird::Driver (DecodeMode::Decode, 48 kHz stereo i16 PCM)
   |  global event handlers registered in songbird_client/packet_handler.rs
   |
   +-- SpeakingStateUpdate ---> ssrc->user_id map, VoiceChannelEvent::UserJoin
   +-- SpeakingUpdate --------> UserAudioEvent{Speaking|Silent} --> tx_voice_activity
   +-- VoicePacket -----------> DiscordAudioData{user_id, i16 samples, rtc ts} --> tx_audio_data
   +-- ClientDisconnect ------> VoiceChannelEvent::UserLeave
   +-- Driver{Connect,Disconnect,Reconnect} --> VoiceChannelEvent::{Connect,Disconnect,Reconnect}

VoiceActivity (songbird_client/voice_activity.rs)
   in : tx_voice_activity
   out: VoiceChannelEvent::ChannelSilent(bool)      (nobody / somebody talking)
        UserAudioEvent{Speaking|Silent|Idle}         -> tx_silent_user_events
        Idle fires USER_SILENCE_TIMEOUT (250 ms) after Silent with no new Speaking

UserAudioManager (scrivening/manager.rs)
   one UserAudioWorker per user_id, created lazily, dropped after 10 min idle
   forwards audio + events to the right worker

UserAudioWorker (scrivening/worker.rs)          <- the interesting loop
   AudioBuffer (audio/audio_buffer.rs): 30 s of 16 kHz mono f32, placed by RTP timestamp
   TranscriptStrategy (strategies/five_second_strategy.rs) decides *when* to run
     whisper and which segments are "final"
   Whisper (audio/whisper.rs): spawn_blocking -> whisper_rs full()
   publishes VoiceChannelEvent::Transcription, then discards the published audio
   and remembers the last TOKENS_TO_KEEP token ids as the prompt for next time

api_task: drains VoiceChannelEvent channel into the user's callback (JSON printer)

Speaker (audio/speaker.rs): stdin line -> espeakng::speak -> rubato resample
   22050 -> 48000 -> songbird Input (raw PCM) -> driver.play_only_source
```

## Key types (`src/model/types.rs`)

`VoiceChannelEvent` is the public API and the JSON wire format. It is a plain
serde externally-tagged enum, e.g. `{"Transcription":{...}}`,
`{"ChannelSilent":true}`, `{"UserJoin":1234}`. `Transcription` contains
`start_timestamp` (`SystemTime`, serialized as
`{"secs_since_epoch","nanos_since_epoch"}`), `user_id`, `segments`
(`TextSegment` with `start_offset_ms`/`end_offset_ms` relative to
`start_timestamp` and `tokens_with_probability`), `audio_duration` and
`processing_time` (`Duration`, serialized as `{"secs","nanos"}`).
**oobabot parses this format; treat it as a compatibility contract.** Add
fields rather than renaming, or version the protocol.

The songbird `ConnectData`/`DisconnectData` types are mirrored by hand here
purely so they can derive `Serialize`.

## Timing constants (`src/model/constants.rs`, `five_second_strategy.rs`, `worker.rs`)

| Constant | Value | Effect |
|----------|-------|--------|
| `AUDIO_TO_RECORD` | 30 s | per-user buffer size; audio beyond the window is dropped |
| `USER_SILENCE_TIMEOUT` | 250 ms | Silent -> Idle delay; also the "silent tail" used to finalize segments |
| `DISCARD_USER_AUDIO_AFTER` | 10 min | idle worker eviction |
| `TOKENS_TO_KEEP` | 1024 | previous token ids fed back as whisper prompt |
| `DONT_EVEN_BOTHER_RMS_THRESHOLD` | 0.01 | audio below this RMS is treated as silence and never sent to whisper |
| `FIRST_TRANSCRIPT_PERIOD` | 5 s | first whisper run after speech starts |
| `SUBSEQUENT_TRANSCRIPT_PERIOD` | 1 s | re-run cadence while speech continues |
| `OUTRAGEOUSLY_MANY_TOKENS` | 100 | segment with this many tokens is discarded as a hallucination |

The doc comments on `TranscriptStrategy::handle_event` (100 ms / 1 s) are
stale; the constants above are what actually runs.

## The five-second strategy, in words

Whisper is run on the whole per-user buffer 5 s after the user starts
talking, then every 1 s, and immediately when songbird reports the user went
silent. Each result is split at (buffer end - 250 ms): segments ending before
that point are published as final and their audio is discarded; the remainder
is kept as a "tentative" transcript. If the user then goes Idle without new
audio, the tentative transcript is published as-is. Whisper output is filtered
by per-token probability (more low-probability than high-probability tokens
discards the segment) and by token count.
