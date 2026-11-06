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

Building needs Espressif's Rust toolchain besides ESP-IDF:
`cargo install espup --locked && espup install -t esp32s3`, then
`. ~/export-esp.sh` in the shell that runs `idf.py`. The component builds the
engine as part of `idf.py build`. On the ESP32-S3 it takes about 330 KB of
flash; a 25-clip mascot holds about 16 KB of heap (43 KB while loading).
`../examples/engine_demo` runs it on any ESP32-S3 board (or ESP-IDF's QEMU,
`idf.py qemu`) and logs what it plays. It sits outside `components/` so the
examples that list that folder do not build it unless they ask for it.
