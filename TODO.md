# TODO

Improvement backlog from the 2026-09-26 code review. Ordered roughly by
severity within each section. Items marked **[voice]** are covered in more
detail in `VOICE-UPDATES.md`.

## 1. Correctness and memory safety

1. **Use-after-free via non-copying `Bytes::from`** in
   `src/audio/audio_buffer.rs` (`get_bytes`) and `src/audio/speaker.rs`
   (`VecMediaSource::new`). `std::slice::from_raw_parts` yields a slice with an
   unbounded lifetime, so the compiler picks `'static` and
   `Bytes::from(&'static [u8])` wraps the pointer without copying.
   - `get_bytes`: the `Bytes` is sent to a `spawn_blocking` whisper job while
     the worker keeps calling `add_audio` (which can `resize`, reallocating)
     and `discard_audio` (which `drain`s). Data race and dangling pointer.
   - `VecMediaSource::new`: the `Vec<i16>` parameter is dropped when the
     function returns, so every playback reads freed memory.
   Fix: copy (`Bytes::copy_from_slice`, or send an owned `Vec<f32>` /
   `Arc<[f32]>`), and drop the byte reinterpretation entirely by passing
   `&[f32]` to whisper directly.
2. **`discrivener-json` busy-loops on stdin EOF.** `read_line` returns
   `Ok(0)` immediately once oobabot closes stdin, the loop trims an empty
   string and calls `speak("")` forever. Break on `Ok(0)`, and ignore blank
   lines. (`examples/discrivener-json.rs`)
3. **Panic paths in shutdown.** `Discrivener::disconnect` does
   `driver.try_lock().unwrap()` twice; the speaker task holds that lock while
   queueing audio. Use `.lock().await`. Same function `unwrap()`s every
   `JoinHandle`; a panicked task turns shutdown into a second panic.
4. **`Speaker::monitor` returns a `JoinHandle` that completes immediately.**
   `run_forever` spawns a *second* task and returns, so the handle stored in
   `Discrivener::speaker` is meaningless and the real loop is only stopped by
   the cancellation token. Make `run_forever` async and spawn it once.
5. **Unbounded `unwrap()` on channel sends** in `packet_handler.rs`
   (`on_user_join`, `on_start_talking`, `on_audio`, `on_stop_talking`,
   `on_user_leave`, `DriverConnect`) and `lib.rs::speak`. During shutdown the
   receivers are gone and songbird may still deliver events, which panics
   inside songbird's event task. Treat `SendError` as "shutting down" like the
   `DriverDisconnect` handler already does.
6. **`Transcription::split_at_end_time` can panic.** In the branch where the
   first half is empty, `.min().unwrap()` panics if every second-half segment
   starts after `end_time` (a segment that begins in the last 250 ms of the
   buffer). The later `start_offset_ms -= first_duration` subtraction can also
   underflow `u32` if whisper returns overlapping segments. Use
   `saturating_sub` and handle the empty case.
7. **Worker task leak after idle eviction.** `UserAudioManager` drops a
   worker's senders after 10 minutes, but the worker's `select!` just disables
   the closed-channel arms and waits on the global shutdown token forever,
   holding its 1.9 MB buffer. Either give each worker a `child_token()` and
   cancel it on eviction, or exit the loop when `recv()` returns `None`.
8. **Worker `Drop` cancels the global token.** `UserAudioWorker` stores a
   clone of the process-wide `CancellationToken`; its `Drop` calls `cancel()`.
   Today workers are never dropped early, but the first refactor that does
   will shut down the whole bot. Use `shutdown_token.child_token()`.
9. **`rms_over_slice` on an empty slice is NaN**, and `NaN < threshold` is
   false, so an empty buffer is sent to whisper. Return 0.0 for empty input.
10. **`UserJoin` fires on every `SpeakingStateUpdate`**, not just the first
    one for a user (Discord sends it whenever mic state toggles). Check whether
    the SSRC was already mapped before emitting. The SSRC map is also never
    cleaned on `ClientDisconnect`.
11. **espeak-ng synthesis blocks a tokio worker thread.** `espeak_Synth` with
    `AUDIO_OUTPUT_RETRIEVAL` is synchronous; the `oneshot` is already resolved
    by the time it is awaited. Wrap in `spawn_blocking`. **[voice]**
12. **`Whisper::load` runs on the async runtime and panics** on a missing
    file. Return `Result` and load in `spawn_blocking` (model load takes
    seconds for larger models). **[voice]**

## 2. Transcription quality and performance **[voice]**

1. Feeding the previous 1024 token ids back as the whisper prompt is a known
   cause of repetition/hallucination loops, and whisper's text context is only
   448 tokens anyway, so most of `TOKENS_TO_KEEP` is discarded. Drop the
   carry-over or replace it with a short `initial_prompt` of vocabulary
   (usernames, bot name).
2. `make_params` never sets `language`, `n_threads`, `no_context`, or
   `single_segment`. On an `.en` model this is harmless, but a multilingual
   model will spend time on language detection every call.
3. No cap on concurrent whisper runs. N talkers means N simultaneous
   `full()` calls on the blocking pool, each spawning whisper.cpp's default
   thread count. Add a `tokio::sync::Semaphore` (or a single inference task
   with a queue) sized to the machine.
4. A new `WhisperState` is created per request. Keep one per worker (or per
   inference slot) and reuse it.
5. Segment validity relies on hand-rolled per-token probability voting
   (`is_valid_segment`) and an RMS gate. Newer whisper.cpp exposes
   `no_speech_prob` per segment and has built-in Silero VAD; use those.
6. `DONT_EVEN_BOTHER_RMS_THRESHOLD`, the 5 s / 1 s cadence and the 250 ms
   silence timeout have no benchmark behind them. Build the replay harness
   (section 5) so these can be tuned against captured audio.
7. `FiveSecondStrategy` re-transcribes the entire per-user buffer on every
   tick (up to 30 s of audio every second while someone talks). A streaming
   model or a sliding-window approach would cut CPU by an order of magnitude.

## 3. Dependency and toolchain upgrades

1. **songbird 0.3.2 -> 0.6.0.** Largest migration; required before Discord's
   DAVE end-to-end encryption rollout makes the old driver unable to connect.
   Changes: `receive` feature, `VoiceTick` replaces `VoicePacket` and
   `SpeakingUpdate`, symphonia `Input` + `RawAdapter` replace `Reader`/`Codec`,
   `opus2` replaces `audiopus`, MSRV 1.83, non-optional channel ids.
   Details in `VOICE-UPDATES.md`, Phase 0.
2. **whisper-rs 0.8.0 -> 0.16.0.** `new_with_params`, `set_suppress_nst`,
   `WhisperSegment` API, GPU features (`cuda`, `vulkan`, `metal`, `hipblas`),
   built-in VAD. **[voice]**
3. tokio 1.28 -> current, rubato 0.14 -> current (API changed to
   `Resampler::process_into_buffer` with `&[&[T]]` slices), serde/serde_json
   patch bumps. Run `cargo update` after the two big ones land.
4. Remove dead dependencies: `serde_with` (`#[serde_as]` is applied but no
   `serde_as` attributes are used), `lazy_static` (use
   `std::sync::LazyLock`), and the `[dev-dependencies.discrivener]`
   self-dependency.
5. Add a `rust-toolchain.toml` or at least document MSRV once songbird 0.6
   (1.83) is in.
6. CI: `actions/checkout@v2/v3` -> v4, replace the unmaintained
   `actions-rs/toolchain` with `dtolnay/rust-toolchain`,
   `github/codeql-action/upload-sarif@v1` -> v3, install `cmake clang
   libclang-dev` explicitly, add a `cargo fmt --check` step, and stop
   `continue-on-error` on clippy once it is clean.

## 4. API and ergonomics

1. Introduce an error type (`thiserror`) and make `Discrivener::load`,
   `connect`, `speak` and `disconnect` return `Result`.
2. Replace `eprintln!` with `tracing` (or `log` + `env_logger`) so oobabot
   users can raise/lower verbosity with `RUST_LOG`. whisper-rs 0.16 has a
   `tracing_backend` feature that routes whisper.cpp logs the same way.
3. Make the tunables in `constants.rs` runtime configuration (a `Config`
   struct passed to `load`, plus CLI flags on `discrivener-json`): model path,
   language, thread count, GPU on/off, silence timeout, buffer length, TTS
   voice.
4. Version the JSON protocol. Add a `{"Version": N}` or `{"Hello": {...}}`
   event at startup so oobabot can detect capability changes when the new
   voice stack lands. Keep existing field names.
5. Remove the unused `save_everything_to_file` CLI flag from both examples,
   or implement it (it would be the natural way to record replay fixtures).
6. `discrivener-cli` puts `#[tokio::main]` on a non-`main` fn; move it to
   `main` or build the runtime explicitly like `discrivener-json` does.
7. The hand-copied songbird `ConnectData`/`DisconnectData` mirrors are only
   there for `Serialize`. Check whether songbird 0.6 types derive serde
   (feature `serde`?) before re-mirroring them.
8. Fix stale comments: strategy trait doc (100 ms / 1 s), "ssid" for SSRC,
   `constants.rs` header describing 20 ms chunks of stereo when the buffer is
   16 kHz mono f32.

## 5. Testing

1. **Replay harness.** `tests/test.json` already holds a captured session
   (songbird events incl. decoded PCM). Write a test that feeds it through
   `PacketHandler`-equivalent code into `UserAudioManager` with a stub or real
   whisper, and assert on the emitted `VoiceChannelEvent`s. This is the
   prerequisite for tuning anything in section 2 and for validating the
   `VOICE-UPDATES.md` model swap.
2. Unit tests for `FiveSecondStrategy` (it is pure and untested) and for
   `is_valid_segment` / `probability_histogram`.
3. Unit test for `resample` (length, no clipping, DC level).
4. `voice_activity.rs` tests use real `sleep`s; switch to
   `#[tokio::test(start_paused = true)]` and `tokio::time::advance` to make
   them deterministic and fast.
5. Add `#![deny(unsafe_op_in_unsafe_fn)]` and document each remaining
   `unsafe` block with a `// SAFETY:` comment once item 1.1 is fixed.

## 6. Packaging and docs

1. README still points at the `v0.0.1` release and the espeak-ng-data tarball;
   update once the TTS engine changes (the tarball goes away with Kokoro).
2. Add a `--version` / `--help` friendly summary of the model formats
   accepted, and a `--list-voices` for TTS.
3. Consider a `justfile`/`Makefile` target for downloading recommended models
   into a `models/` directory (gitignored).
