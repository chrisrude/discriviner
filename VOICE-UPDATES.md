# VOICE-UPDATES.md

Plan for replacing the 2023-era speech-to-text (whisper.cpp base.en via
whisper-rs 0.8) and text-to-speech (espeak-ng) stack with current models.
Written 2026-09-26. Version numbers below were checked against crates.io and
upstream docs on that date.

## Goals

1. Noticeably better transcription accuracy and lower time-to-first-text,
   on CPU by default, with optional GPU acceleration.
2. A natural-sounding voice instead of espeak-ng's formant synthesis.
3. No change to the stdout JSON contract that `oobabot` parses (additive
   fields only), and a single `discrivener_model_location`-style setting that
   keeps working.
4. Keep the "one static binary + model files" deployment story.

## Current state (what is being replaced)

| Layer | Today | Problem |
|-------|-------|---------|
| Discord voice | songbird 0.3.2 | pre-dates DAVE E2EE, `VoicePacket` API removed upstream, unmaintained `audiopus` |
| STT engine | whisper-rs 0.8.0 (whisper.cpp 1.4.2) | 12 major versions behind; no GPU features that still build; no VAD |
| STT model | `ggml-base.en` recommended | high WER vs. current small models |
| Segmentation | 5 s / 1 s full-buffer re-transcription + RMS gate + token-probability voting | expensive, heuristic, hallucination-prone (prompt carry-over) |
| TTS engine | espeak-ng via `espeakng-sys` (git dep, bindgen) | robotic voice; needs external `espeak-ng-data`; global mutable state; blocks the runtime |
| Resampling | rubato 0.14 | fine, just old |

## Options considered

### Speech-to-text

| Option | Rust path | Streaming | Quality (EN) | CPU cost | Notes |
|--------|-----------|-----------|--------------|----------|-------|
| **Whisper large-v3-turbo (ggml, q5_0 ~550 MB)** | whisper-rs 0.16 | no (chunked) | very good, 99 langs | ~4-6x faster than large-v3; realtime on 8+ cores, easily on GPU | smallest code change; keeps ggml model files |
| Whisper small.en / distil-whisper | whisper-rs 0.16 | no | good | low | drop-in if turbo is too slow on target hardware |
| **NVIDIA Parakeet TDT 0.6B (v2 EN / v3 25 langs)** | `sherpa-onnx` crate | no (but very fast chunks) | best-in-class EN WER on open leaderboards | very low per second of audio | CC-BY-4.0; ONNX runtime dep |
| Moonshine v2 (tiny/base) | `sherpa-onnx` crate | yes | ~Whisper-base/small | very low | built for edge/streaming |
| Zipformer streaming transducer (EN) | `sherpa-onnx` crate | yes, token-level | good | low | true incremental output + endpointing |
| Kyutai STT 1B/2.6B | separate Rust server (candle), websocket | yes | very good | GPU only | wrong shape for an embedded library |
| Cloud (Deepgram, OpenAI realtime, etc.) | HTTP/WS client | yes | excellent | none locally | violates the local-first design; not planned |

### Text-to-speech

| Option | Rust path | Quality | Latency / cost | License | Notes |
|--------|-----------|---------|----------------|---------|-------|
| **Kokoro-82M** | `sherpa-onnx` `OfflineTts` (also `kokoroxide`, `kokorox` crates) | high, natural | faster than realtime on CPU, ~300 MB model | Apache-2.0 | 24 kHz output; many EN voices; the obvious fit |
| Piper (VITS) | `sherpa-onnx` `OfflineTts` | medium | very fast on CPU, ~60 MB | MIT | most languages; fallback for non-English guilds |
| Chatterbox-Turbo / Orpheus / Kyutai TTS | Python or GPU-only servers | best | GPU | MIT / Apache | not embeddable in this crate today |
| Cloud (ElevenLabs, OpenAI tts) | HTTP | best | network | paid | not planned |

### Why `sherpa-onnx`

The official `sherpa-onnx` Rust crate (1.13.x, Apache-2.0, published by the
k2-fsa project; the older third-party `sherpa-rs` is deprecated) gives us, from
one native dependency:

- offline recognizers: Whisper, Parakeet, Moonshine, SenseVoice, Canary, Qwen3-ASR, ...
- online (streaming) recognizers: Zipformer transducer, Paraformer, ...
- Silero VAD
- offline TTS: Kokoro, Piper/VITS, Matcha, ...

Its build script downloads a matching prebuilt native library from GitHub
releases unless `SHERPA_ONNX_LIB_DIR` is set, so it removes the cmake +
clang + bindgen requirements that `whisper-rs-sys` and `espeakng-sys` impose.
It can replace **both** engines. The trade-off is a ~30-50 MB ONNX Runtime
shared/static lib and a build-time network fetch (mirror it in CI).

## Recommended plan

Phased so each step ships independently and the JSON contract never breaks.

### Phase 0: unblock the build (prerequisite for everything)

1. Install native prerequisites on the dev box (`cmake clang libclang-dev
   pkg-config`) and get `cargo build --examples` green on the current lock
   file so there is a known-good baseline.
2. Fix the memory-safety bugs listed in `TODO.md` 1.1 first; the new STT
   path will touch the same code and it should not inherit the aliasing
   `Bytes`.
3. **songbird 0.3.2 -> 0.6.0.** Do this as its own PR.
   - Enable features `driver`, `receive`, `rustls`.
   - Replace the `VoicePacket` + `SpeakingUpdate` handlers with one
     `CoreEvent::VoiceTick` handler. `VoiceTick` fires every 20 ms with
     `speaking: HashMap<ssrc, VoiceData { packet: Option<RtpData>, decoded_voice: Option<Vec<i16>> }>`
     and `silent: HashSet<ssrc>`. Derive the current `Speaking`/`Silent`
     events from membership changes between ticks (this also lets
     `USER_SILENCE_TIMEOUT` be measured in ticks instead of a `BinaryHeap`
     of timers).
   - Set `Config::decode_channels(1)` and `decode_sample_rate(16000)` if
     available in 0.6 so songbird's Opus decoder emits mono 16 kHz directly
     and `resample_audio_from_discord_to_whisper` disappears. Otherwise keep
     the existing 48 kHz stereo -> 16 kHz mono downmix.
   - Playback: replace `Reader::Extension(VecMediaSource)` + `Codec::Pcm` with
     `RawAdapter::new(source, 48000, 1)` over interleaved **f32** PCM and
     `driver.play_only_input(adapter.into())`.
   - `ConnectionInfo::channel_id` is no longer `Option`.
   - `audiopus` -> `opus2` happens transitively; check whether cmake is still
     needed for libopus.
4. Build the replay harness (`TODO.md` 5.1) from `tests/test.json` so the
   before/after of every later phase can be measured: WER against a
   hand-corrected transcript, time from end-of-speech to `Transcription`
   event, CPU seconds per audio second.

### Phase 1: modernize whisper (low risk, ships first)

whisper-rs 0.8.0 -> 0.16.0 with model and parameter changes. Expected
result: large accuracy improvement, similar or better CPU cost with
`large-v3-turbo-q5_0`, GPU optional.

API migration in `src/audio/whisper.rs`:

| 0.8 | 0.16 |
|-----|------|
| `WhisperContext::new(path)` | `WhisperContext::new_with_params(path, WhisperContextParameters { use_gpu, flash_attn, gpu_device, .. })` |
| `params.set_suppress_non_speech_tokens(true)` | `params.set_suppress_nst(true)` |
| `state.full_n_segments()` + `full_get_segment_t0/t1`, `full_n_tokens`, `full_get_token_{text,id,prob}` | `for seg in state.as_iter()` -> `WhisperSegment::{start_timestamp, end_timestamp, to_str_lossy, n_tokens, get_token(i), no_speech_probability}`; token via `WhisperToken` accessors |
| `WhisperToken` (i32 alias) | `WhisperTokenId` |
| `Cargo.toml` `features = ["cuda"]` | `cuda` / `vulkan` / `metal` / `hipblas` / `openblas` / `openmp` / `tracing_backend` |

Parameter changes:

- `set_language(Some("en"))` (configurable), `set_n_threads(min(8, cores))`,
  `set_no_context(true)`, `set_single_segment(false)`, keep greedy sampling.
- Stop feeding `TOKENS_TO_KEEP` ids via `set_tokens`; optionally pass a short
  `set_initial_prompt` with the bot's name and channel vocabulary.
- Enable whisper.cpp's built-in Silero VAD (`params.enable_vad(true)`,
  `set_vad_model_path(...)`, `set_vad_params(...)`). Ship
  `ggml-silero-v5.1.2.bin` (~1 MB) alongside the model. This replaces
  `DONT_EVEN_BOTHER_RMS_THRESHOLD` and most of `is_valid_segment`.
- Use `WhisperSegment::no_speech_probability()` (drop segments > 0.6) instead
  of the token-probability vote.
- Add `flash_attn(true)` when `use_gpu` is on.

Model handling:

- Recommend `ggml-large-v3-turbo-q5_0.bin` (~550 MB) as the default in the
  README; `ggml-small.en-q5_1.bin` for weak CPUs. Both are on
  huggingface.co/ggerganov/whisper.cpp. Existing `ggml-base.en` files keep
  working unchanged.
- Add `--threads`, `--gpu`, `--language` flags to `discrivener-json`.

Cargo features to expose on the `discrivener` crate: `cuda`, `vulkan`,
`metal`, forwarding to whisper-rs, so release binaries can be built per
target.

Tests: replay harness numbers before/after; the unit tests in
`audio_buffer.rs` are unaffected.

### Phase 2: STT engine abstraction and a streaming backend

1. Introduce `trait SttEngine` (in `src/audio/stt/mod.rs`):
   ```rust
   pub(crate) trait SttEngine: Send + Sync {
       fn transcribe(&self, req: TranscriptionRequest) -> JoinHandle<TranscriptionResponse>;
       fn is_streaming(&self) -> bool;
   }
   ```
   with the whisper implementation moved behind it. `TranscriptionRequest`
   carries an owned `Arc<[f32]>` at 16 kHz mono, never `Bytes`.
2. Add a `sherpa-onnx` backend (`features = ["sherpa"]`) with:
   - **Parakeet TDT 0.6B v2** as the offline recognizer (best English WER,
     ~10x cheaper than whisper per second of audio). Model is a directory of
     ONNX files; select by passing a directory instead of a `.bin` to the
     existing model-location setting.
   - **Silero VAD** from the same crate feeding the worker's endpointing, so
     the strategy publishes on VAD end-of-utterance rather than on the 250 ms
     songbird silence heuristic.
   - Optionally the **Zipformer streaming transducer** as an
     `is_streaming() == true` engine that emits partial text every ~300 ms.
     This would need a new additive event, e.g.
     `{"PartialTranscription": {...}}`, which oobabot can ignore until it
     supports it.
3. Rework `FiveSecondStrategy` into a VAD-driven strategy: transcribe each
   utterance once when VAD closes it (plus a forced flush at 15-20 s for
   monologues). This removes the 1 s re-transcription loop and the
   tentative/finalized split logic in `Transcription::split_at_end_time`.
4. Add a `Semaphore` around inference so N talkers cannot start N parallel
   jobs on the blocking pool.
5. Decide the default engine from replay-harness numbers on a typical
   oobabot host (CPU-only VPS and a desktop GPU are the two profiles).

### Phase 3: text-to-speech with Kokoro

1. Introduce `trait TtsEngine { fn synthesize(&self, text: &str) -> Result<Audio> }`
   where `Audio { sample_rate: u32, samples: Vec<f32> }`, run on
   `spawn_blocking`.
2. Implement it with `sherpa-onnx` `OfflineTts` and the Kokoro-82M ONNX model
   (`kokoro-en-v0_19` or the current multilingual `kokoro-multi-lang-v1_x`
   package from the sherpa-onnx releases). Default voice `af_heart`
   (configurable `--voice`, `--speed`). If Phase 2 did not adopt sherpa, the
   `kokoroxide` / `kokorox` crates are lighter alternatives with the same
   model.
3. Resample 24 kHz -> 48 kHz with rubato (bump to current 0.16.x API) and
   hand songbird a `RawAdapter` over f32 samples. Chunk long messages by
   sentence and queue them as separate tracks so the first sentence starts
   playing before the rest is synthesized.
4. Keep espeak-ng behind a `features = ["espeak"]` flag for one release for
   non-English guilds that Kokoro does not cover, then remove it along with
   the `espeak-ng-data` tarball instructions in the README.
5. Interrupt handling: expose a `stop_speaking` control (new stdin command,
   e.g. a line starting with `\x01` or a JSON object) so oobabot can cut the
   bot off when a user starts talking. songbird's `TrackHandle::stop()`
   makes this trivial once tracks are used.

### Phase 4: cleanup and release

- Delete `audio/espeakng.rs`, `audio/resample.rs` (if songbird decodes to
  16 kHz and sherpa resamples TTS), the `Bytes` reinterpretation helpers, and
  the `lazy_static` dependency.
- Update README: new model download table, hardware guidance, flags.
- Cut release binaries per feature set (cpu, cuda, vulkan, metal) in CI.
- Bump to 0.3.0 and tell oobabot users which config keys changed.

## Effort estimate

| Phase | Size | Notes |
|-------|------|-------|
| 0 | 2-4 days | songbird migration dominates; harness is ~1 day |
| 1 | 1-2 days | mechanical API changes plus tuning on the harness |
| 2 | 3-5 days | trait, sherpa backend, VAD strategy rewrite |
| 3 | 2-3 days | Kokoro + playback path |
| 4 | 1 day | |

## Open questions

- Which hardware profile is the primary target? If most oobabot users run on
  the same GPU box as the LLM, Phase 1 with `cuda`/`vulkan` may already be
  enough and Phase 2 can be deferred.
- Does oobabot need partial transcripts (for barge-in / wakeword) or only
  finals? That decides whether the streaming Zipformer engine is worth it.
- Is multilingual support a requirement? It affects the Parakeet v2 vs. v3
  and Kokoro vs. Piper choices.
- Model distribution: bundle a downloader in `discrivener-json`, or keep
  telling users to fetch files by hand?

## References

- whisper-rs 0.16.0 (2026-03-12): https://docs.rs/whisper-rs/latest/whisper_rs/
- songbird 0.6.0 (2026-04-05) release notes: https://github.com/serenity-rs/songbird/releases
- sherpa-onnx Rust crate: https://docs.rs/sherpa-onnx/latest/sherpa_onnx/
- sherpa-onnx pretrained models: https://k2-fsa.github.io/sherpa/onnx/pretrained_models/index.html
- ggml whisper models: https://huggingface.co/ggerganov/whisper.cpp/tree/main
- Kokoro TTS crates: https://lib.rs/crates/kokoroxide , https://lib.rs/crates/kokorox
- Kyutai STT/TTS (for reference): https://kyutai.org/stt/ , https://kyutai.org/tts/
