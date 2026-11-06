# Measured calibration — ESP32-S3R8 / Waveshare 1.75" AMOLED

Every number here was **measured on real hardware** (Waveshare ESP32-S3-Touch-AMOLED-1.75,
ESP-IDF 5.5.5, toolchain esp-14.2.0_20260121), not estimated. Use these as defaults in
`masko_core/include/masko_calib.h` rather than the estimates in the original plan.

## Where the plan was wrong

| Quantity | Plan estimate | **Measured** | Delta |
|---|---|---|---|
| JPEG decode throughput | 3.2–4.1 Mpx/s | **7.4 Mpx/s** | plan was 2× pessimistic |
| Decode 466×466 | 58–68 ms | **29.4 ms** | 2× faster |
| Full-frame push 466×466 | ≈22 ms | **26.5 ms** | 20% slower |
| Free internal DRAM at boot | not measured | **310,979 B** | — |
| Effective QSPI throughput | 20 MB/s (theoretical) | **16.4 MB/s** | 18% of theoretical lost to protocol overhead |

The plan's decode estimate was the most conservative input in the whole design, and it
came from GIF/older-library benchmarks. `esp_new_jpeg` uses Xtensa SIMD (see
`IDCT_BLOCK_8_8_128ASM` in a real backtrace) and is roughly twice as fast.

## Per-frame costs at 466×466, q=9, serial (no overlap)

| Stage | Measured |
|---|---|
| `esp_jpeg` decode → RGB565 | 29,403 µs (29.4 ms) |
| Banded push (10 × 50 rows) | 26,542 µs (26.5 ms) |
| **Serial total** | **55,945 µs → 17.9 fps** |

Note the push, not the decode, is the larger term at full screen size. That inverts the
plan's assumption that decode would be the binding constraint.

## Scaling by clip size

Decode scales with pixels. Push scales with bytes actually sent, which for a
bounding-box-cropped clip is `bbox_w × bbox_h × 2`.

| Size | Pixels | Decode @7.4 Mpx/s | Push @16.4 MB/s | Serial fps | Overlapped fps |
|---|---|---|---|---|---|
| 466×466 | 217,156 | 29.4 ms | 26.5 ms | 17.9 | ~27 |
| 360×360 | 129,600 | 17.5 ms | 15.8 ms | 30.1 | 41 |
| 320×320 | 102,400 | 13.8 ms | 12.5 ms | 38.0 | 51 |

**Overlapped** assumes decode and push run concurrently (`max()` of the two), which is
what `masko_display`'s double-buffer presenter does.

## Hard constraints discovered the hard way

### 1. The SPI DMA buffer must fit in internal SRAM

`esp_lcd_panel_draw_bitmap` makes the SPI master allocate a **private DMA buffer of
`max_transfer_sz` bytes from internal DRAM**. A full 466×466 frame is 434,312 B but only
310,979 B of internal DRAM was free, so a whole-frame push fails:

```
E spi_master: setup_dma_priv_buffer(1214): Failed to allocate priv TX buffer
E lcd_panel.io.spi: panel_io_spi_tx_color(405): spi transmit (queue) color failed
W anim_player: Panel push failed: ESP_ERR_NO_MEM
```

The screen stays black and the error is easy to miss among the JPEG work. **Cap
`max_transfer_sz` at ~46,600 B (50 rows) and push in bands.** Total bytes over the bus are
unchanged; only the allocation granularity changes.

### 2. RGB565 byte order is big-endian for this panel

Use `JPEG_PIXEL_FORMAT_RGB565_BE`, **not** `_LE`. `esp_jpeg` writes both bytes into the
buffer in memory order and the CO5300 consumes them in that order over QSPI, expecting the
high byte first. The chip's own endianness is irrelevant.

With `_LE` the yellow background `(238, 206, 57)` renders as light green `(131, 251, 115)`.
This is a colour corruption, not a brightness shift, and it is easy to misread as
"the wrong panel".

### 3. Container alpha tags are unreliable in both directions

| File | `stream.pix_fmt` | Container tag | Actual alpha |
|---|---|---|---|
| webm (VP9) | `yuv420p` | `alpha_mode=1` | **none** (single alpha value, 255) |
| mov (HEVC) | `yuv420p` | `alpha_mode=none` | **full** (256 distinct values) |

Two files reporting the *same* pixel format, one opaque and one transparent. **Decode a
frame and inspect the alpha plane.** Never branch on `pix_fmt` or on an `alpha_mode` tag.

### 4. PSRAM→DMA works at full speed on this board

Open question #6 in the plan is answered: no: pushing directly from a PSRAM frame buffer
costs the same 26.5 ms as anything else, and produced no visible artefacts. **Bounce strips
into internal RAM are not needed.**

### 5. JPEG quality dominates RGB565 quantisation

Measured against a 24-bit reference of the same clip:

| Conversion | PSNR |
|---|---|
| 24-bit → RGB565 (panel limit alone) | 42.8 dB |
| 24-bit → q=9 JPEG alone | 37.6 dB |
| both (what the panel shows) | 36.8 dB |

**The panel's 16-bit limit costs less than the JPEG.** Raising JPEG quality is the only
lever that meaningfully improves the image; there is no point spending effort on RGB565
dithering.

### Quality ladder (24-bit reference, 124 frames, flattened on black)

| `-q:v` | Bytes | KB/frame | PSNR | SSIM |
|---|---|---|---|---|
| 2 | 2.71 MB | 22.3 | — (reference) | 1.000 |
| 3 | 2.15 MB | 17.7 | 47.4 dB | 0.993 |
| 5 | 1.63 MB | 13.5 | 45.2 dB | 0.990 |
| **9** (default) | **1.18 MB** | **9.8** | **41.4 dB** | **0.980** |
| 15 | 0.90 MB | 7.4 | 38.7 dB | 0.969 |
| 20 | 0.77 MB | 6.4 | 37.1 dB | 0.960 |

ffmpeg's MJPEG scale runs 1–31, so `-q:v 9` is the midpoint, not "high quality".
`-q:v 85` in the plan's §2.3 is a **libjpeg quality scale and does not apply to ffmpeg's
`-q:v`**; translate it to roughly `-q:v 4` to get comparable output.

## Panel geometry

| Item | Value |
|---|---|
| Resolution | 466×466, RGB565 |
| Driver clock | `pclk_hz = 40 MHz`, quad mode (set by `CO5300_PANEL_IO_QSPI_CONFIG`) |
| Column address gap | **6** (`esp_lcd_panel_set_gap(panel, 0x06, 0)` — set by the BSP) |
| Row address gap | 0 |
| Driver | CO5300 v2.2.0 (QSPI AMOLED), **not** SH8601 |
| Device | ESP32-S3R8: 8 MB octal PSRAM @80 MHz, 16 MB flash, 310 KB internal DRAM free |

The CO5300 driver takes normal pixel coordinates (`esp_lcd_panel_draw_bitmap(x0,y0,x1,y1)`
with `x1`/`y1` exclusive) and applies the gap itself. The `2N+1` rounding callback in the
BSP is conditionally compiled and **not** active for this panel.

## Toolchain

ESP-IDF 5.5.5 requires `xtensa-esp-elf esp-14.2.0_20260121`. If `export.sh` activates
`esp-15.2.0_20251204` the build aborts with a tool-version error. Pin with
`idf_tools.py select-version xtensa-esp-elf esp-14.2.0_20260121`, or prefix the correct
toolchain's `bin` onto `PATH` for a single build.

## ESP-IDF 5.5 API moves

| 5.4 | 5.5 |
|---|---|
| `#include "esp_lcd_panel.h"` | `"esp_lcd_types.h"` (handle typedef) + `"esp_lcd_panel_ops.h"` (`draw_bitmap`) |
| `EMBED_FILES "assets/x.mjpeg"` → `_binary_assets_x_mjpeg_start` | basename only → `_binary_x_mjpeg_start` |
| `esp_jpeg.h` | `esp_jpeg_dec.h` / `esp_jpeg_enc.h` |