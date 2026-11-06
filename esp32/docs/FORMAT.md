# `.mkpk` v1 — Masko ESP32 pack format

A pack is a self-describing, randomly-indexable container of already-decoded-friendly
frames. The device never sees Masko's native formats, and never runs a real video decoder.

Little-endian throughout.

## Header (64 bytes)

| Offset | Type | Field | Notes |
|---|---|---|---|
| 0 | `char[4]` | magic | `"MKPK"` |
| 4 | `u16` | format_version | `1` |
| 6 | `u16` | header_size | `64` |
| 8 | `u8` | codec | 1 = JPEG baseline, 2 = RGB565 raw, 3 = RGB565 delta+LZ4 (reserved) |
| 9 | `u8` | flags | bit0 loop, bit1 alpha plane, bit2 premultiplied |
| 10 | `u8` | jpeg_subsampling | 0 = 4:2:0, 1 = 4:2:2, 2 = 4:4:4, 3 = gray |
| 11 | `u8` | reserved | 0 |
| 12 | `u16` | canvas_w | panel width the clip was authored for |
| 14 | `u16` | canvas_h | panel height |
| 16 | `u16` | bbox_x | **even** |
| 18 | `u16` | bbox_y | **even** |
| 20 | `u16` | bbox_w | **multiple of 16** (4:2:0 MCU) |
| 22 | `u16` | bbox_h | **multiple of 16** |
| 24 | `u16` | fps_num | |
| 26 | `u16` | fps_den | usually 1 |
| 28 | `u32` | frame_count | |
| 32 | `u32` | index_offset | absolute |
| 36 | `u32` | data_offset | absolute, 512-byte aligned |
| 40 | `u32` | file_size | total bytes |
| 44 | `u16` | bg_rgb565 | background as `0xBBGGRR` per RGB565_BE |
| 46 | `u16` | reserved | 0 |
| 48 | `u32` | payload_crc32 | crc32 of the frame index + payload |
| 52 | `u32` | header_crc32 | crc32 of bytes 0..51, so the header is self-checking |
| 56 | `u8[8]` | reserved | 0 |

## Frame index

`frame_count` entries of `{u32 offset, u32 length}`, **absolute** offsets into the file so a
reader can mmap flash and index without arithmetic. Followed by the payload.

## Payload

Each frame starts on a **512-byte boundary** (SD sector friendly, trivially mappable in
flash), padded with zeros.

Frame bytes are ordinary baseline JPEG with `FFD8` at the start and `FFD9` at the end, so
a pack can be inspected with `dd` and `ffprobe`.

## Compatibility rules

- Reject unknown `format_version`.
- Ignore unknown `flags` bits and all `reserved` fields.
- A reader must not assume `bbox` fills the canvas.

## Why the geometry constraints exist

`bbox_x`/`bbox_y` must be even and `bbox_w`/`bbox_h` multiples of 16 so that:

1. 4:2:0 chroma subsampling never straddles a crop edge,
2. update rectangles passed to the CO5300 are already aligned, so the runtime never
   rounds and never needs to re-align a half-pixel window.

Cropping to the union bounding box is what makes smaller clips cheap: decode time scales
with `bbox_w × bbox_h`, and a mascot that occupies 70% of the square gains ≈1.4× in fps.

## Companion files

| File | Purpose |
|---|---|
| `graph.json` | trimmed `MaskoAnimationConfig`: nodes, edges, inputs, `initialNode`, `autoPlay` |
| `manifest.json` | pack version, per-file `{name, bytes, sha256}`, Masko `graph_content_hash`, tool version, encode parameters |

## Storage variants

**Flash partition.** A raw `masko` data partition: a table of contents of
`{name, offset, size}`, then 512-byte-aligned `.mkpk` files. The runtime
`esp_partition_mmap`s it and decodes straight from the mapping. Never write the partition
while a clip is playing — flash writes stall execution.

**SD card.** One `.mkpk` per file. At ~10 KB/frame a 5 s clip is ~1.2 MB, well under the
~1 MB/s a 1-bit SDMMC bus sustains.