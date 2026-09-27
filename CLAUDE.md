# CLAUDE.md

Working notes for anyone (human or AI) doing future work on this repository.
Companion documents: `TODO.md` (prioritized improvement backlog from the
2026-09-26 code review) and `VOICE-UPDATES.md` (plan for modernizing the
speech-to-text and text-to-speech stack).

## What this is

`discrivener` is a Rust library plus two example binaries that join a Discord
voice channel, transcribe what each user says with whisper.cpp, and can speak
text back into the channel with espeak-ng. It is consumed as a **subprocess**
by the Python project `oobabot` (`discrivener_location` /
`discrivener_model_location` in oobabot's `config.yml`). The library does not
talk to the Discord gateway itself: the caller obtains the voice connection
details (endpoint, session id, voice token) some other way and passes them in.

- Crate version `0.2.0`, MIT license, edition 2021.
- Last substantive commit: 2023-06-27. Dependencies are pinned to mid-2023
  versions in `Cargo.lock` (songbird 0.3.2, whisper-rs 0.8.0, tokio 1.28).
- `.gitignore` excludes `target/` and `ggml-*.bin` (models live in the repo
  root during development).

## Build and run

Native build prerequisites (none of the pure-Rust deps are the hard part):

| Need | Why |
|------|-----|
| `cmake` | `audiopus_sys` builds libopus from source; `whisper-rs-sys` builds whisper.cpp |
| `clang` + `libclang` | `espeakng-sys` uses bindgen (`clang-runtime` feature) and compiles espeak-ng |
| C/C++ toolchain | all of the above |
| `espeak-ng-data` at runtime | `espeak_Initialize` is called with a null data path, so it uses the compiled-in default (`/usr/local/share/espeak-ng-data`). The README's release tarball provides this. |
| a ggml Whisper model file | passed as the first positional argument to both examples |

On the WSL2 dev machine used for the 2026-09-26 review, `cmake`, `clang`,
`pkg-config` and `espeak-ng` were all absent and `cargo check --examples`
failed in the `audiopus_sys` build script. Install them before expecting
anything to compile:

```bash
sudo apt-get install cmake clang libclang-dev pkg-config build-essential
```

Commands:

```bash
cargo build --examples            # library + discrivener-json + discrivener-cli
cargo test                        # unit tests only (see Testing)
cargo clippy --all-features
cargo run --example discrivener-json -- <model.bin> -c <channel_id> -e <endpoint> -g <guild_id> -s <session_id> -u <user_id> -v <voice_token>
```

`discrivener-json` is the binary oobabot runs. Its contract:

- **stdout**: one JSON object per line, one per `VoiceChannelEvent`.
- **stdin**: one line per message to speak aloud in the channel.
- **stderr**: free-form debug logging (`eprintln!` everywhere; there is no
  `log`/`tracing` integration).
- Exits on Ctrl-C.

`discrivener-cli` is a human-readable variant of the same thing and does not
support speaking.

## Architecture

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

### Key types (`src/model/types.rs`)

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

### Timing constants (`src/model/constants.rs` and strategy files)

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

### The five-second strategy, in words

Whisper is run on the whole per-user buffer 5 s after the user starts
talking, then every 1 s, and immediately when songbird reports the user went
silent. Each result is split at (buffer end - 250 ms): segments ending before
that point are published as final and their audio is discarded; the remainder
is kept as a "tentative" transcript. If the user then goes Idle without new
audio, the tentative transcript is published as-is. Whisper output is filtered
by per-token probability (more low-probability than high-probability tokens
discards the segment) and by token count.

## Threading and safety notes

- Whisper inference runs on tokio's blocking pool, one job per user at a time
  (the worker only submits when `pending_transcription_requests` is empty),
  but nothing limits concurrency *across* users. Two talkers means two whisper
  runs at once, each using whisper.cpp's default thread count.
- `Whisper::load` and `espeakng::init` panic on failure; there is no error
  type anywhere in the crate. Most channel `send()`s are `unwrap()`ed.
- `unsafe` lives in exactly four places: `AudioBuffer::get_bytes`,
  `Whisper::audio_to_text` (f32 <-> byte reinterpretation), `VecMediaSource::new`
  (i16 -> bytes) and the espeak-ng FFI. **The `Bytes::from(slice)` calls in
  `get_bytes` and `VecMediaSource::new` do not copy** (the raw slice gets an
  inferred `'static` lifetime), so those `Bytes` alias memory that is later
  mutated or freed. See TODO.md, item 1. Do not build on this pattern.
- espeak-ng is a process-wide singleton behind a `lazy_static` mutex;
  `speak()` asserts no other synthesis is in flight. Only `Speaker` calls it.
- The `Drop` impl on `UserAudioWorker` cancels its token, and that token is a
  clone of the *global* shutdown token, not a child. Dropping a worker early
  would shut the whole process down.
- `Discrivener::disconnect` uses `try_lock().unwrap()` on the driver mutex and
  will panic if the speaker task holds the lock at that moment.

## Testing

- Unit tests exist in `audio_buffer.rs` (timestamp placement, discard, silence
  detection), `voice_activity.rs` (state machine, uses real sleeps) and
  `types.rs` (`split_at_end_time`). Run with `cargo test`.
- `tests/test.json` is 1.1 MB of captured songbird events (SpeakingStateUpdate,
  SpeakingUpdate, VoicePacket with decoded audio). Nothing reads it today; it
  is the natural fixture for a replay harness (see TODO.md).
- There are no integration tests and nothing exercises whisper or espeak-ng
  under `cargo test`. Verifying transcription changes means running an example
  against a real voice channel or building the replay harness first.
- CI (`.github/workflows/rust.yml`) runs `cargo build --examples` and
  `cargo test` on `ubuntu-latest`; the clippy workflow uses deprecated
  actions (`actions-rs`, `upload-sarif@v1`) and is marked `continue-on-error`.

## Conventions observed in the code

- Modules are declared inline in `lib.rs`; only `model::types` and the
  `Discrivener` struct are `pub`, everything else is `pub(crate)`.
- Each long-lived component follows the same shape: a `monitor(...)` /
  `register(...)` constructor that spawns a task and returns a `JoinHandle` or
  channel senders, and a private `loop_forever` with a `tokio::select!` over
  the shutdown token and its input channels.
- Logging is `eprintln!` to stderr. Keep stdout clean in `discrivener-json`;
  oobabot parses it.
- Comments frequently say "ssid" where the code means SSRC.
- Formatting is rustfmt default; clippy was last run clean in June 2023 and
  the current toolchain (1.98) will have new lints.

## Things that will bite you when upgrading dependencies

- songbird 0.4+ removed `CoreEvent::VoicePacket` and `SpeakingUpdate`, gated
  receive behind the `receive` feature, and replaced the `Reader`/`Codec`
  input API with symphonia-based `Input` + `RawAdapter`. 0.6 adds DAVE
  end-to-end encryption and moves from `audiopus` to `opus2`. This is the
  largest migration in the codebase; see VOICE-UPDATES.md, Phase 0.
- whisper-rs 0.16 replaced `WhisperContext::new` with `new_with_params`,
  renamed `set_suppress_non_speech_tokens` to `set_suppress_nst`, and moved
  segment/token access to `WhisperState::get_segment` / `as_iter()` returning
  `WhisperSegment`. Timestamps are still centiseconds.
- `[dev-dependencies.discrivener] path = "./"` in `Cargo.toml` is a
  self-dependency and can be deleted; examples already see the library.
