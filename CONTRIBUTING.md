
# Trying to get this to build

The module was never built except on an old version of OSX in
2023, and since then there was a lot of version drift.

## Nominal Building Steps

Install rust / cargo

sudo apt install cmake
sudo apt install clang llvm lldb lld

try to build with `cargo build`.

## Fixing Build Failures

The first failure is with `audiopus_sys-0.2.2`,
which depends on a no-longer-supported version of cmake.

This can be fixed with a cli flag to the new cmake.
I monkey-patched the build script to pass this.

The location for the crate build on this system is in
`.cargo/registry/src/index.crates.io-<something>`

To patch the audiopus build:

```diff
diff --git a/audiopus_sys-0.2.2/build.rs b/audiopus_sys-0.2.2/build.rs
index 6fd28d6..116ef27 100644
--- a/audiopus_sys-0.2.2/build.rs
+++ b/audiopus_sys-0.2.2/build.rs
@@ -47,7 +47,10 @@ fn build_opus(is_static: bool) {
     );
 
     println!("cargo:info=Building Opus via CMake.");
-    let opus_build_dir = cmake::build(opus_path);
+    // let opus_build_dir = cmake::build(opus_path);
+    let opus_build_dir = cmake::Config::new(opus_path)
+        .define("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
+        .build();
     link_opus(is_static, opus_build_dir.display())
 }
```

Then run `cargo clean` to delete the earlier failed build directory,
so that cargo will copy over the patched files.  (we needed the build
failure first to get the crate source downloaded in the first place).

Then run `cargo build` again, so that more progress can be made, stopping
at the next failure: `whisper-rs-sys-0.6.1`.  Same deal.

Patch `whisper-rs-sys-0.6.1`'s build in the same way:

```diff
diff --git a/whisper-rs-sys-0.6.1/build.rs b/whisper-rs-sys-0.6.1/build.rs
index 1c58968..1944936 100644
--- a/whisper-rs-sys-0.6.1/build.rs
+++ b/whisper-rs-sys-0.6.1/build.rs
@@ -112,6 +112,8 @@ fn main() {
     cmd.arg("-DWHISPER_CLBLAST=ON");
 
     cmd.arg("-DCMAKE_POSITION_INDEPENDENT_CODE=ON");
+    // hack: cmake 3.5 support is deprecated, but might work if we pass this
+    cmd.arg("-DCMAKE_POLICY_VERSION_MINIMUM=3.5");
 
     let code = cmd
         .status()
```

Then run `cargo clean` again, and `cargo build`.

This time it fails with:

```bash
error: failed to run custom build command for `espeakng-sys v0.1.2 (https://github.com/Better-Player/espeakng-sys/#2b686c7e)`

...

  thread 'main' (66746) panicked at /home/rude/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/proc-macro2-1.0.63/src/fallback.rs:791:9:
  "__mbstate_t_union_(unnamed_at_/usr/include/x86_64-linux-gnu/bits/types/__mbstate_t_h_16_3)" is not a valid Ident
```
