# Porting to another panel

The measured constants in `masko_calib.h` are specific to the Waveshare 1.75" CO5300.
To move to a different display, work through this list.

## 1. Geometry and transfer size

```c
#define MASKO_PANEL_W        <width>
#define MASKO_PANEL_H        <height>
#define MASKO_PUSH_BAND_ROWS <rows per esp_lcd call>
#define MASKO_PUSH_BAND_BYTES (MASKO_PANEL_W * MASKO_PUSH_BAND_ROWS * 2)
```

`MASKO_PUSH_BAND_BYTES` becomes `bsp_display_config_t.max_transfer_sz`. It must fit in
free internal DRAM, not PSRAM. If pushes fail with `ESP_ERR_NO_MEM`, halve the band.

## 2. Pixel format

Most SPI panels are `RGB565_BE`. I2C panels are usually `RGB565_LE`. **Determine this
empirically**: encode a frame with known colours, push it, and look. The symptom of
getting it wrong is a colour cast, not a crash.

## 3. Address gaps

Some panels need an offset, e.g. the CO5300 here uses
`esp_lcd_panel_set_gap(panel, 0x06, 0)`. A wrong gap shifts the image horizontally or
vertically without otherwise breaking it.

## 4. Re-measure, do not trust the defaults

Build the `masko_demo` example and read the periodic stats line:

```
decode avg 29403 / max 29040 us | push avg 26542 us | serial ceiling 18.0 fps
```

Then update:

```c
#define MASKO_DECODE_MPX_S   <pixels / decode_us * 1e6>
#define MASKO_PUSH_BYTES_PER_S <bbox_bytes / push_us * 1e6>
```

`masko-pack build` prints an estimate using these constants; if it disagrees with the
device, the constants are stale.

## 5. Check block mode

`esp_jpeg` block mode streams 8- or 16-line strips and would let you decode and push
without a full-frame buffer. It requires **both dimensions to be multiples of 8**. A
466-wide panel is not; a cropped 336×432 bbox is (432 is, 336 is). If your bbox happens
to be a multiple of 8, try `block_enable = true`.

## 6. Alpha

Panels without an alpha channel need the host to flatten. If your panel genuinely supports
alpha, keep the alpha plane, set `MASKO_FLAG_ALPHA`, and add a compositing step in
`masko_display_blit`.

## Other platforms

The pack format and `masko_core` are portable. On an **ESP32-P4** the hardware JPEG
decoder sits behind a different API; implement `masko_decoder_decode()` against it and
the packs stay unchanged.
