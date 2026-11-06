# Masko ESP32 SDK

An ESP-IDF SDK for preparing and playing Masko mascot animations on ESP32 devices. It
converts source video on a development computer into compact `.mkpk` animation packs;
the device decodes baseline JPEG frames and sends RGB565 pixels to an injected display
panel.

> **Development status:** this project is still evolving and does not promise a stable
> API or compatibility between revisions. Waveshare ESP32-S3-Touch-AMOLED-1.75 playback
> and touch-driven switching have been verified on hardware. The SD source builds but
> has not yet been validated on hardware. See the component status below before choosing
> a storage path for a product.

## Features

- Host-side `.mkpk` build, inspect, preview, and verification tools.
- ESP-IDF components for pack reading, JPEG decoding, display output, playback, storage,
  and Masko canvas graphs.
- Decode and display work can run on separate cores, with configurable frame-rate and
  banded display transfer limits.
- Flash, memory, and SD pack sources through `masko_store` (SD hardware validation is
  pending).
- Reference Waveshare example with multi-pack bundles, authored transitions, touch, and
  BOOT-button playback.

## Quick start: Waveshare reference board

### Requirements

- ESP-IDF 5.3.1 or newer; developed and measured with ESP-IDF 5.5.5.
- ESP32-S3 board with PSRAM and at least 4 MB flash. The reference board has 8 MB PSRAM
  and 16 MB flash.
- Python 3, `ffmpeg`, and `ffprobe` on your computer. `numpy` is optional and speeds up
  mascot bounding-box measurement.
- The Waveshare ESP32-S3-Touch-AMOLED-1.75 board for the example as written.

### Build and flash the example

The example discovers the SDK components from this checkout. Activate your ESP-IDF
environment, then run:

```bash
cd examples/waveshare_amoled_175
idf.py set-target esp32s3
idf.py build
idf.py -p PORT flash monitor
```

Replace `PORT` with your serial port (for example, `/dev/cu.usbmodem101` on macOS or
`/dev/ttyACM0` on Linux). The example includes a sample animation in its assets and
uses the Waveshare board support package.

### Prepare and install your own animation

Convert a video supported by `ffmpeg` into a pack sized for the reference 466 × 466
display. `--crop` trims the image to the mascot's visible bounds; `--bg` composites any
transparency onto a solid background before the file reaches the device.

```bash
tools/masko_pack.py build mascot.mov --fps 24 --quality 9 --bg 000000 --crop -o loop.mkpk
tools/masko_pack.py verify loop.mkpk
tools/flash_pack.sh loop.mkpk
```

`flash_pack.sh` writes the pack to the example's `masko` data partition. Connect the
board and flash the application (without erasing the pack partition):

```bash
cd examples/waveshare_amoled_175
idf.py -p PORT flash monitor
```

The Waveshare transition demo bundles seven packs in this exact order. Touch or press the
onboard BOOT button to react: from Waving, the demo cycles through Thinking, Typing (the
laptop animation), and Moonwalk. Press again in any reaction to return to Waving. Return
transitions reuse their forward transition packs in reverse on-device:

```bash
python tools/masko_bundle.py mascot_bundle.bin \
  assets/waving.loop.mkpk assets/typing.loop.mkpk assets/thinking.loop.mkpk \
  assets/moonwalk.loop.mkpk assets/waving_to_typing.mkpk \
  assets/waving_to_thinking.mkpk assets/waving_to_moonwalk.mkpk \
  --once assets/waving_to_typing.mkpk --once assets/waving_to_thinking.mkpk \
  --once assets/waving_to_moonwalk.mkpk
python "$IDF_PATH/components/partition_table/parttool.py" \
  --port PORT write_partition --partition-name masko --input mascot_bundle.bin
```

All seven packs must use the same decoded bounding-box dimensions. The Masko exports use
464×464 frames centered on the 466×466 display. Waving, Typing, Thinking, and Moonwalk
remain continuous loops. Transition packs are one-shot; after a transition completes,
playback continues with the destination loop. Touch and the onboard BOOT button on GPIO0
use the same reaction handler. Listening is not part of this demo bundle.

## Use the components in an ESP-IDF project

The SDK is organized as local ESP-IDF components. Clone this repository alongside your
application and add its `components` directory to `EXTRA_COMPONENT_DIRS` before including
`project.cmake`:

```cmake
cmake_minimum_required(VERSION 3.16)
set(EXTRA_COMPONENT_DIRS "${CMAKE_CURRENT_LIST_DIR}/../esp32-masko-sdk/components")
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(my_app)
```

Declare the components your app uses in its component registration. The reference
example's `main/CMakeLists.txt` is a working starting point:

```cmake
idf_component_register(
    SRCS "main.c"
    INCLUDE_DIRS "."
    REQUIRES masko_player masko_display masko_codec masko_core masko_store masko_graph
)
```

Initialize the board's display panel, create a display adapter, open a pack through a
source, and pass the pack bytes to the player. The exact board initialization and source
setup depend on your project:

```c
masko_display_cfg_t display_cfg = {
    .panel = panel,               // initialized esp_lcd_panel_handle_t
    .canvas_w = 466,
    .canvas_h = 466,
    .band_rows = MASKO_PUSH_BAND_ROWS,
};
masko_display_t *display = NULL;
ESP_ERROR_CHECK(masko_display_new(&display_cfg, &display));

masko_source_t *source = NULL;
ESP_ERROR_CHECK(masko_src_flash("masko", &source));

masko_blob_t blob;
ESP_ERROR_CHECK(masko_source_open(source, "idle.mkpk", &blob));
masko_player_cfg_t player_cfg = {
    .pack_data = blob.data,
    .pack_size = blob.len,
    .display = display,
    .decode_core = 1,
};
masko_player_t *player = NULL;
ESP_ERROR_CHECK(masko_player_create(&player_cfg, &player));
ESP_ERROR_CHECK(masko_player_play(player));
```

Consult the public headers in `components/*/include` and the complete reference app for
event callbacks, cleanup, display initialization, and graph integration. The display
component accepts an `esp_lcd_panel_handle_t`; it does not depend on the Waveshare BSP.

## Storage and memory

`masko_store` provides pack sources for a flash partition, SD card, or an existing memory
buffer:

```c
masko_src_flash("masko", &source);                 // memory-mapped partition
masko_src_sd("/sdcard/idle.mkpk", 0, &source);     // reads pack into PSRAM
masko_src_memory(data, length, &source);            // caller-owned buffer
```

Packs are opened as complete blobs. An SD pack therefore needs enough PSRAM for the
whole pack; the SD backend rejects an undersized cache instead of returning truncated
data. Flash playback is verified. SD playback has not yet been exercised on hardware.
Do not write a flash partition while playing a pack from it: flash writes can stall
execution and visibly interrupt playback.

The player uses banded panel transfers to limit internal DMA memory. Avoid allocating a
full RGB565 frame in internal DRAM: a 466 × 466 frame is about 434 KB, while the display
driver’s DMA buffer must come from a much smaller internal-memory pool. Keep large
animation data in mapped flash or PSRAM and tune band size against your panel and memory
budget.

### Authored transition playback

Use a Masko transition asset between loop clips instead of crossfading transparent
frames. `masko_player_set_pack()` validates the new pack and decodes its first frame into
a dedicated PSRAM handoff buffer before returning. The current clip keeps playing until
its loop boundary; a non-looping pack holds its final frame and emits
`MASKO_EV_CLIP_END` once. The controller can then queue the next pack from that event.
The prepared frame is presented after already queued outgoing frames, and the player
continues with the new clip without clearing the display.

In the `MASKO_EV_CLIP_END` callback, ask the graph/controller for the next edge and
call `masko_player_set_pack()` with that edge's pack. Queue the transition after its
source loop ends; queue the target loop when the one-shot transition ends.

The player requires all clips in a sequence to share the same decoded bounding-box
dimensions and display placement. The preflight buffer adds one RGB565 frame in PSRAM
and uses a separate decoder for candidate validation; the two playback buffers and
banded display transfer remain unchanged. A failed candidate keeps the current picture
and reports `MASKO_EV_ERROR`.

## Masko canvas graphs

`masko_graph` executes canvas nodes, edges, inputs, and conditions. Its state machine is
independent of ESP-IDF; `masko_graph_json` loads a Masko animation configuration using
cJSON.

```c
masko_graph_json_t parsed;
if (masko_graph_json_parse(json_text, json_len, &parsed) != MASKO_GRAPH_OK) {
    // Handle the invalid canvas; release partial parser allocations if needed.
}

masko_graph_t graph;
if (masko_graph_json_bind(&parsed, &graph) != MASKO_GRAPH_OK) {
    masko_graph_json_free(&parsed);
    // Handle bind failure.
}

masko_graph_rt_t runtime;
if (masko_graph_init(&runtime, &graph, NULL, NULL, 0) == MASKO_GRAPH_OK) {
    masko_graph_set_bool(&runtime, "happy", true);

    masko_step_t step;
    while (masko_graph_next(&runtime, &step) == MASKO_GRAPH_OK) {
        // step.edge->clip identifies the clip to play; transitions are one-shot.
    }
    masko_graph_free(&runtime);
}
masko_graph_json_free(&parsed);
```

At a loop boundary, eligible non-loop edges are checked in declaration order and the
first edge whose conditions hold is selected. A transition plays once before the graph
lands on its target node. Invalid references, undeclared inputs, unsupported operators,
and nodes without loop edges are rejected during parsing.

## Tools and documentation

| Path | Purpose |
|---|---|
| `tools/masko_pack.py` | Build, inspect, preview, and verify `.mkpk` files |
| `tools/masko_bundle.py` | Bundle two or more compatible packs for the reference demo |
| `tools/flash_pack.sh` | Write a pack to the example's flash partition |
| `tools/test_all.sh` | Run host-side tests without a board |
| [`docs/FORMAT.md`](docs/FORMAT.md) | `.mkpk` v1 file format |
| [`docs/CALIBRATION.md`](docs/CALIBRATION.md) | Hardware measurements and display behavior |
| [`docs/PORTING.md`](docs/PORTING.md) | Port the display path to another panel |

Run the host tests with:

```bash
tools/test_all.sh
tools/test_all.sh path/to/animation.mkpk
tools/masko_pack.py inspect path/to/animation.mkpk
tools/masko_pack.py verify path/to/animation.mkpk
```

The graph JSON tests use cJSON from ESP-IDF when `IDF_PATH` is set, or system cJSON
(`libcjson-dev` on Debian/Ubuntu). Pack-conversion tools additionally require `ffmpeg`
and `ffprobe`.

## Performance and display notes

On the Waveshare ESP32-S3-Touch-AMOLED-1.75 with ESP-IDF 5.5.5, the verified playback
path runs at about 24 fps with no observed dropped frames or underruns. Actual results
depend on clip dimensions, panel transfer speed, and concurrent application work.
Cropping around the mascot is the most useful first optimization; it reduces both JPEG
decode work and pixels sent to the display. The default frame cap is 24 fps.

The reference panel requires RGB565 big-endian pixel output and banded QSPI transfers.
Transparency must be flattened to a background color by the host packer because RGB565
has no alpha channel. The `--crop` option aligns bounds for JPEG's 4:2:0 blocks. See
[`docs/CALIBRATION.md`](docs/CALIBRATION.md) and [`docs/PORTING.md`](docs/PORTING.md)
before adapting the display path.

## License

The SDK is licensed under the MIT License. Exported Masko assets may be subject to
Masko's terms; check those terms before shipping them in a product.
