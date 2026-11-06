# Engine size

Measured on 2026-10-24 for the full engine (every rule of `spec/rules.md`).

## On the web

Built by `scripts/build-web.sh`: the `masko-player-wasm` crate, wasm-bindgen
`--target web`, then `wasm-opt -Oz`.

| File                              |       Raw |  gzip -9 | brotli -q 11 |
| --------------------------------- | --------: | -------: | -----------: |
| Engine `.wasm`, after `wasm-opt`  | 187,530 B | 77,037 B | **64,767 B** |
| JavaScript glue from wasm-bindgen |  23,991 B |  3,802 B |      3,407 B |

So the engine ships as about **65 KB of WebAssembly plus 3 KB of
JavaScript** over the wire. One 360x360 mascot loop weighs 118 KB to 1 MB,
so the engine costs less than a single clip. The one-file script build
(`web/dist/masko-player.js`) inlines it.

## On Android

`libmasko_player_ffi.so`, built by `scripts/build-android.sh`, before the
store compresses it:

| ABI         |      Size |
| ----------- | --------: |
| arm64-v8a   | 663,248 B |
| armeabi-v7a | 444,032 B |
| x86_64      | 755,336 B |

## On ESP32 boards

Measured on 2026-11-06: `bindings/c` built without the standard library
(`--no-default-features`, `xtensa-esp32s3-none-elf`) and linked into
`esp32/examples/engine_demo` with ESP-IDF 5.5.5, then
`idf.py size-components`:

| Part of the firmware | Flash code | Flash data | Static RAM |
| -------------------- | ---------: | ---------: | ---------: |
| `libmasko_player_c.a` | 149,911 B | 12,789 B | 0 B |

So about 163 KB of the 16 MB of flash of an ESP32-S3 board. Built with the
standard library (`xtensa-esp32s3-espidf`) it was 312 KB. At run time the
test canvas `otto` holds about 5 KB of heap.

## Making the web build smaller

Most of the WebAssembly is JSON decoding: serde's generated code for the
delivery types, serde_json's parser and its error messages, and string
formatting. The rules themselves are a small part. Three levers, in order of
effect:

1. Let JavaScript parse the delivery (`JSON.parse`) and hand the engine a
   JavaScript object through `serde-wasm-bindgen`, dropping serde_json's
   parser from the web build only.
2. Report load errors as codes plus an edge id instead of formatted strings.
3. Keep `wasm-opt -Oz` in the build (about 5%).

## How it was built

- Rust 1.99.1 (`rust-toolchain.toml`), release profile from the workspace
  `Cargo.toml` (`opt-level = "z"`, LTO, one codegen unit, `panic = "abort"`,
  stripped). The Android build overrides `panic` to unwind
  (`scripts/build-android.sh`).
- wasm-bindgen 0.2.129, serde 1.0.229, serde_json 1.0.154.
- binaryen 132 `wasm-opt -Oz`, from `web/node_modules`.
