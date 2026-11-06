# masko_engine

The Masko player engine for ESP32 firmware: every canvas rule of
`spec/rules.md`, the same Rust code as the web, Apple and Android players,
called from C through `bindings/c` (`masko_engine.h`). It decides which clip
plays; the firmware plays it (the `.mkpk` components beside this one) and
reports when it shows and ends.

```c
char *error = NULL;
masko_engine_t *engine = masko_engine_load(json, len, &error);
char *commands = masko_engine_start(engine, now_ms);
/* [{"type":"poseEntered","pose":"rest"},{"type":"showClip","edge":"loop-rest","kind":"pose","rate":1.0}, ...] */
masko_string_free(commands);
commands = masko_engine_set_bool(engine, "behavior::working", true, false, now_ms);
commands = masko_engine_clip_ended(engine, "loop-rest", now_ms);
```

`json` is a release delivery: `data.delivery` of
`GET https://api.masko.ai/v1/canvases/{canvasId}/delivery?target=esp32`, or the
`mascot.json` of the SD card bundle (`GET /v1/canvases/{canvasId}/bundle`).
Each `showClip` names an edge; its pack is the edge's `videos.mkpk`. In an
esp32 delivery, an edge with `reverse: true` carries the pack of the move it
returns from, to play backwards.

## Using it

Add this folder to `EXTRA_COMPONENT_DIRS` and `REQUIRES masko_engine`.
`../examples/engine_demo` does that: it plays a test canvas on any ESP32-S3
board, or in ESP-IDF's QEMU (`idf.py qemu`), and logs each clip and the heap.

With a compiled engine for the chip in `lib/<chip>/libmasko_player_c.a`, the
firmware builds with ESP-IDF alone. The ESP32 workflow
(`.github/workflows/esp32.yml`) compiles one for the ESP32-S3, ESP32, ESP32-S2,
ESP32-C3 and ESP32-C6 on every change and keeps each as a download named
`masko_engine-<chip>`; unpack it into this folder. Compiled files stay out of
git.

Without one, the component compiles the engine during `idf.py build`. The
ESP32-C3 and ESP32-C6 need Rust with their target
(`rustup target add riscv32imc-unknown-none-elf` or
`riscv32imac-unknown-none-elf`). The ESP32-S3, ESP32 and ESP32-S2 need
Espressif's Rust: `cargo install espup --locked && espup install -t esp32s3`.
Do not load `~/export-esp.sh` in the shell that runs `idf.py`: its newer GCC
comes first on the path, and ESP-IDF refuses a GCC that is not the version it
ships. The component only runs `cargo +esp`.

## How it is built

The engine builds without Rust's standard library (`--no-default-features`),
so it works with any ESP-IDF version: from the firmware it takes only
`malloc`, `memalign`, `free` and `abort`, and a crash inside it calls `abort`
without a message. On the ESP32-S3 it takes about 163 KB of flash and no
static RAM; the test canvas holds about 5 KB of heap, and a 25-clip mascot
about 16 KB (43 KB while it loads).

This component sits outside `components/` on purpose: the examples there list
that whole folder, and would otherwise build the engine without using it.
ESP-IDF's embedded text files end with a zero byte, which
`masko_engine_load` ignores.
