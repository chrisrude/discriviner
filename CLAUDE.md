# CLAUDE.md

Working notes for anyone (human or AI) doing future work on this repository.
Companion documents: `docs/ARCHITECTURE.md` (data flow, key types, timing
constants, transcription strategy), `TODO.md` (prioritized improvement
backlog from the 2026-09-26 code review) and `docs/VOICE-UPDATES.md` (plan for
modernizing the speech-to-text and text-to-speech stack).

## What this is

`discrivener` is a Rust library plus two example binaries that join a Discord
voice channel, transcribe what each user says with whisper.cpp, and can speak
text back into the channel with espeak-ng. It is consumed as a **subprocess**
by the Python project `oobabot` (`discrivener_location` /
`discrivener_model_location` in oobabot's `config.yml`). The library does not
talk to the Discord gateway itself: the caller obtains the voice connection
details (endpoint, session id, voice token) some other way and passes them in.

- Crate version `0.2.0`, MIT license, edition 2021.
- Last substantive code commit: 2023-06-27. Dependencies are pinned to
  mid-2023 versions in `Cargo.lock` (songbird 0.3.2, whisper-rs 0.8.0,
  tokio 1.28), except for the build-only changes described under "Build and
  run" (2026-10-01) that were needed to compile with a 2026 toolchain.
- `.gitignore` excludes `target/` and `ggml-*.bin` (models live in the repo
  root during development).

## Build and run

See `CONTRIBUTING.md` for the step-by-step setup. Native build prerequisites
(none of the pure-Rust deps are the hard part):

| Need | Why |
|------|-----|
| Rust >= 1.85 | `espeakng-sys` 0.3.0 uses edition 2024; `Cargo.lock` is format v4 |
| `cmake` | `audiopus_sys` builds libopus from source; `whisper-rs-sys` builds whisper.cpp |
| `clang` (provides `libclang`) | bindgen in `espeakng-sys` (`clang-runtime` feature, loads libclang at build time) and `whisper-rs-sys` |
| C/C++ toolchain (`build-essential`) | all of the above |
| `libespeak-ng-dev` | `espeakng-sys` does **not** build espeak-ng; it ships its own headers and links the system `libespeak-ng`. Without it, the library compiles but the examples and `cargo test` fail to link (`unable to find library -lespeak-ng`). |
| `espeak-ng-data` at runtime | installed as a dependency of `libespeak-ng-dev`. `espeak_Initialize` is called with a null data path, so it uses the library's compiled-in default; for Ubuntu's package that is `/usr/lib/x86_64-linux-gnu/espeak-ng-data`. |
| a ggml Whisper model file | passed as the first positional argument to both examples |

```bash
sudo apt install build-essential clang cmake libespeak-ng-dev
```

Workarounds already in the repo for building old dependencies with a 2026
toolchain (CMake 4.2, clang 21, rustc 1.98, Ubuntu 26.04 under WSL2):

- `.cargo/config.toml` sets `CMAKE_POLICY_VERSION_MINIMUM=3.5` in the build
  environment. CMake 4 refuses the `cmake_minimum_required` versions used by
  the bundled opus (`audiopus_sys` 0.2.2) and whisper.cpp (`whisper-rs-sys`
  0.6.1) without it. Do not patch crates under `~/.cargo/registry` instead.
- `espeakng-sys` is pinned by git `rev` to `b71489b`, the last commit of the
  now-archived upstream repo (version 0.3.0). The previously locked commit
  used bindgen 0.59, which panics with clang 16+ (`"..._(unnamed_at_/usr/include/...)"
  is not a valid Ident`). The upstream changes between the two commits are
  only the bindgen bump and the edition; the headers are unchanged.
- bindgen 0.71 needs `proc-macro2` >= 1.0.80 but doesn't declare it, so the
  lockfile is bumped to 1.0.80 by hand. If it regresses, the build fails with
  `no associated function ... c_string`; fix with
  `cargo update -p proc-macro2 --precise 1.0.80`.
- Newer bindgen maps `size_t` to `usize`, which is why `espeak_Synth` in
  `src/audio/espeakng.rs` takes `str_bytes.len()` with no cast.

Commands:

```bash
cargo build --examples            # library + discrivener-json + discrivener-cli
cargo test                        # unit tests only (see Testing)
cargo clippy --all-features
cargo run --example discrivener-json -- <model.bin> -c <channel_id> -e <endpoint> -g <guild_id> -s <session_id> -u <user_id> -v <voice_token>
```

`discrivener-json` is the binary oobabot runs: one JSON `VoiceChannelEvent`
per line on stdout, one line of text to speak per line on stdin, debug
logging on stderr. `discrivener-cli` is a human-readable variant without
speaking. Full contract in `docs/ARCHITECTURE.md`.

## Architecture

Read `docs/ARCHITECTURE.md` before changing anything in the audio or
transcription path. It has the source layout, the task/channel data-flow
diagram, the `VoiceChannelEvent` wire format, the timing constants table and
a prose description of the five-second strategy. The short version:

- Tokio tasks connected by unbounded mpsc channels, all stopped by one shared
  `CancellationToken`. `Discrivener::load` in `src/lib.rs` wires everything.
- Pipeline: songbird event handlers -> `VoiceActivity` (speaking/silent/idle)
  -> `UserAudioManager` -> one `UserAudioWorker` per user (30 s
  `AudioBuffer` + `FiveSecondStrategy` + whisper) -> `VoiceChannelEvent` ->
  caller's callback. TTS is a separate `Speaker` task (espeak-ng -> rubato
  -> songbird).
- **`VoiceChannelEvent`'s serde JSON (`src/model/types.rs`) is a
  compatibility contract with oobabot.** Add fields rather than renaming, or
  version the protocol.
- Tunables live in `src/model/constants.rs` and
  `src/strategies/five_second_strategy.rs` (plus `OUTRAGEOUSLY_MANY_TOKENS`
  in `src/scrivening/worker.rs`). The 100 ms / 1 s figures in the
  `TranscriptStrategy::handle_event` doc comments are stale.

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
  mutated or freed. See `TODO.md`, item 1. Do not build on this pattern.
- espeak-ng is a process-wide singleton behind a `lazy_static` mutex;
  `speak()` asserts no other synthesis is in flight. Only `Speaker` calls it.
- The `Drop` impl on `UserAudioWorker` cancels its token, and that token is a
  clone of the *global* shutdown token, not a child. Dropping a worker early
  would shut the whole process down.
- `Discrivener::disconnect` uses `try_lock().unwrap()` on the driver mutex and
  will panic if the speaker task holds the lock at that moment.

## Testing

- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each file:
  `audio_buffer.rs` (timestamp placement, discard, silence detection),
  `voice_activity.rs` (state machine, paused tokio clock via the
  `test-util` dev-feature), `types.rs` (`split_at_end_time`),
  `five_second_strategy.rs`, `worker.rs` (segment filtering, histogram,
  token buffer) and `resample.rs`. Run with `cargo test`.
- Tests that reproduce a known, not-yet-fixed bug are marked
  `#[ignore = "known bug: ..."]` so the suite stays green. `cargo test --
  --ignored` lists them (they should all fail); when fixing the bug, remove
  the `ignore`.
- `tests/test.json` is 1.1 MB of captured songbird events (SpeakingStateUpdate,
  SpeakingUpdate, VoicePacket with decoded audio). Nothing reads it today; it
  is the natural fixture for a replay harness (see `TODO.md`).
- There are no integration tests and nothing exercises whisper or espeak-ng
  under `cargo test`. Verifying transcription changes means running an example
  against a real voice channel or building the replay harness first.
- CI (`.github/workflows/rust.yml`) installs `libespeak-ng-dev`, then runs
  `cargo build --examples` and `cargo test` on `ubuntu-latest`. The runner
  image supplies cmake, clang and the Rust toolchain, so it is not testing
  the exact 2026 toolchain described above. The clippy workflow uses deprecated
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
- **No references to `.md` files in code** (source, comments, build
  scripts, config). The code must stand on its own: put the explanation in
  the comment itself rather than pointing at `TODO.md`, `docs/`, etc. The
  docs may reference code, not the other way around.
- Build-time workarounds go in repo-level config (`.cargo/config.toml`,
  `Cargo.toml` pins, `Cargo.lock`), never in patched copies of crate sources.

## Things that will bite you when upgrading dependencies

- songbird 0.4+ removed `CoreEvent::VoicePacket` and `SpeakingUpdate`, gated
  receive behind the `receive` feature, and replaced the `Reader`/`Codec`
  input API with symphonia-based `Input` + `RawAdapter`. 0.6 adds DAVE
  end-to-end encryption and moves from `audiopus` to `opus2`. This is the
  largest migration in the codebase; see `docs/VOICE-UPDATES.md`, Phase 0.
- whisper-rs 0.16 replaced `WhisperContext::new` with `new_with_params`,
  renamed `set_suppress_non_speech_tokens` to `set_suppress_nst`, and moved
  segment/token access to `WhisperState::get_segment` / `as_iter()` returning
  `WhisperSegment`. Timestamps are still centiseconds.
- `[dev-dependencies.discrivener] path = "./"` in `Cargo.toml` is a
  self-dependency and can be deleted; examples already see the library.
