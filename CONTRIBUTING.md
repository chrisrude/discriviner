# Building

note: this was only tested on Ubuntu 26.04, so ymmv.

## System Requirements

To start, install rust / cargo (rustup is easiest), Rust 1.85 or newer.

Several dependencies build bundled C/C++ libraries (opus, whisper.cpp)
or generate bindings with bindgen, so we need a C compiler stack,
cmake, and libclang.

We link against the system espeak-ng library, so install:

- libespeak-ng-dev

All together:

```bash
sudo apt install build-essential clang cmake libespeak-ng-dev
```

## Nominal Building Steps

```bash
cargo build --examples
cargo test
```

`.cargo/config.toml` sets `CMAKE_POLICY_VERSION_MINIMUM=3.5`, which
`audiopus_sys` and `whisper-rs-sys` need to configure under CMake 4.
No patching of crates in `~/.cargo/registry` is needed.

## Notes on pinned dependencies

- `espeakng-sys` is pinned by git `rev` to the final commit of the
  (now archived) upstream repo, which uses a bindgen new enough to
  handle clang 16+.
- `proc-macro2` is locked at 1.0.80 or newer because bindgen 0.71
  needs it but doesn't declare it.  If the lockfile is ever regenerated
  and the build fails with `no associated function ... c_string`, run
  `cargo update -p proc-macro2 --precise 1.0.80`.
