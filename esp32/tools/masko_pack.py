#!/usr/bin/env python3
"""
masko-pack — build, inspect, preview and verify `.mkpk` packs.

Converts a source clip (anything ffmpeg can read, including transparent video) into
the container described in docs/FORMAT.md.

Design notes that matter on-device:

  * Alpha is detected by DECODING a frame, never from `pix_fmt` or an `alpha_mode`
    container tag. Two real files in testing reported the same `pix_fmt=yuv420p` while
    one was fully opaque and the other fully transparent.
  * Frames are composited onto a solid background here, at zero runtime cost. The
    CO5300 is RGB565 and has no alpha, so transparency cannot be displayed on device
    and must never reach it.
  * ffmpeg's `-q:v` runs 1..31 (1 = best). It is NOT libjpeg's 0..100 quality scale.
  * The bbox is rounded to multiples of 16 so 4:2:0 chroma never straddles a crop edge
    and the runtime can push the rectangle without re-alignment.

Usage:
    masko_pack.py build INPUT [-o OUT] [--fps 24] [--quality 9] [--bg 000000]
                             [--size 466] [--crop] [--loop]
    masko_pack.py inspect PACK
    masko_pack.py preview PACK -o preview.mp4
    masko_pack.py verify  PACK
"""

from __future__ import annotations

import argparse
import binascii
import json
import os
import struct
import subprocess
import sys
import zlib

MAGIC = b"MKPK"
FORMAT_VERSION = 1
HEADER_SIZE = 64
PAYLOAD_ALIGN = 512

CODEC_JPEG = 1
CODEC_RGB565 = 2

FLAG_LOOP = 1 << 0
FLAG_ALPHA = 1 << 1
FLAG_PREMULTIPLIED = 1 << 2

SUBSAMPLING_420 = 0
SUBSAMPLING_422 = 1
SUBSAMPLING_444 = 2

MCU = 16  # 4:2:0 MCU; bbox dimensions must be a multiple of this

TOOL_VERSION = "1.0.0"


# --------------------------------------------------------------------------- #
# ffmpeg helpers
# --------------------------------------------------------------------------- #

def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, capture_output=True, **kw)


def ffprobe_json(path: str) -> dict:
    out = run(["ffprobe", "-v", "error", "-print_format", "json",
               "-show_format", "-show_streams", path]).stdout
    return json.loads(out)


def video_stream(info: dict) -> dict:
    for s in info["streams"]:
        if s.get("codec_type") == "video":
            return s
    raise SystemExit("no video stream found")


def _duration(path: str) -> float:
    """Source duration in seconds; used to bound every generated colour source."""
    try:
        d = ffprobe_json(path).get("format", {}).get("duration")
        return float(d) if d else 10.0
    except Exception:
        return 10.0


def probe_resolution(path: str) -> tuple[int, int]:
    s = video_stream(ffprobe_json(path))
    return int(s["width"]), int(s["height"])


def has_real_alpha(path: str) -> bool:
    """Ground-truth alpha detection: decode one frame, look at the alpha plane.

    Container tags lie in both directions -- a WebM tagged alpha_mode=1 may be fully
    opaque, and a MOV tagged alpha_mode=none may carry a full alpha plane.
    """
    proc = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path,
         "-vf", "select=eq(n\\,0)", "-vframes", "1",
         "-pix_fmt", "rgba", "-f", "rawvideo", "-"],
        capture_output=True)
    data = proc.stdout
    if len(data) < 4:
        return False
    alpha = data[3::4]
    return bool(alpha) and min(alpha) < 255


def read_scalars_and_ensure_rgb565_be() -> None:
    """No-op placeholder: the byte-order decision lives on the device."""


# --------------------------------------------------------------------------- #
# composition
# --------------------------------------------------------------------------- #

def parse_colour(spec: str) -> tuple[int, int, int]:
    """Accept `000000`, `#000000`, `0x000000` or a CSS name via ffmpeg."""
    s = spec.strip().lstrip("#")
    if s.lower().startswith("0x"):
        s = s[2:]
    if len(s) == 6:
        try:
            return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))
        except ValueError:
            pass
    if len(s) == 8:  # RRGGBBAA
        try:
            return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))
        except ValueError:
            pass
    # fall back to ffmpeg's colour parser
    png = subprocess.run(
        ["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=c={spec}:s=2x2",
         "-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"],
        capture_output=True).stdout
    if len(png) >= 3:
        return (png[0], png[1], png[2])
    raise SystemExit(f"cannot parse colour: {spec!r}")


def build_filter(width: int, height: int, fps: int, bg: str, alpha: bool,
                 crop: tuple[int, int, int, int] | None) -> list[str]:
    """Compose the filtergraph.

    For alpha sources we overlay onto a solid colour. Note `overlay` must keep its
    default output format: forcing `format=auto` or an explicit RGB format routes the
    composite through limited-range YUV and halves the output range.
    """
    chain = (f"[1:v]fps={fps},"
             f"scale={width}:{height}:flags=lanczos,"
             f"setsar=1")
    if crop:
        x, y, w, h = crop
        chain += f",crop={w}:{h}:{x}:{y}"
    if not alpha:
        return ["-i", None] if False else [f"{chain},format=yuvj420p[out]"]

    chain += ",format=rgba[fg]"
    return [
        chain + ";",
        f"[0:v][fg]overlay=x=0:y=0,format=yuvj420p[out]",
    ]


def measure_bbox(path: str, width: int, height: int, fps: int, bg: str,
                 alpha: bool) -> tuple[int, int, int, int, int]:
    """Union bounding box of everything that differs from the background.

    Returns (x, y, w, h, used_pixels). Uses a coarse scan because we only need the
    extremes, not an exact mask.
    """
    bg_r, bg_g, bg_b = bg
    vf = f"fps={fps},scale={width}:{height}:flags=lanczos,setsar=1,format=rgba"
    cmd = ["ffmpeg", "-v", "error"]
    if alpha:
        # -t is mandatory: a bare `color=` source is infinite, so overlay would
        # emit frames forever and the build would appear to hang.
        cmd += ["-f", "lavfi", "-t", str(_duration(path)), "-i",
                f"color=c=0x{bg_r:02x}{bg_g:02x}{bg_b:02x}:s={width}x{height}:r={fps}"]
    cmd += ["-i", path]
    if alpha:
        g = (f"[1:v]fps={fps},scale={width}:{height}:flags=lanczos,setsar=1,"
             f"format=rgba[fg];[0:v][fg]overlay=x=0:y=0,format=rgb24[out]")
        cmd += ["-filter_complex", g, "-map", "[out]"]
    else:
        cmd += ["-vf", f"{vf},format=rgb24"]
    cmd += ["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]

    # Stream frame by frame. Capturing the whole 124-frame raw stream costs ~81 MB,
    # and the int16 numpy comparison doubles that before it is freed -- enough to
    # get OOM-killed. Only the running extremes are needed.
    try:
        import numpy as np
    except ImportError:
        np = None

    bgv = np.array([bg_r, bg_g, bg_b], dtype=np.int16) if np is not None else None
    frame = width * height * 3

    minx, miny, maxx, maxy = width, height, -1, -1
    count = 0

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert proc.stdout is not None
    while True:
        buf = proc.stdout.read(frame)
        if not buf or len(buf) < frame:
            break
        count += 1
        if np is not None:
            img = np.frombuffer(buf, dtype=np.uint8).reshape(height, width, 3)
            mask = np.abs(img.astype(np.int16) - bgv).max(axis=2) > 6
            cols = np.nonzero(mask.any(axis=0))[0]
            if len(cols):
                rows = np.nonzero(mask.any(axis=1))[0]
                x0, x1 = int(cols.min()), int(cols.max())
                y0, y1 = int(rows.min()), int(rows.max())
                minx = min(minx, x0); maxx = max(maxx, x1)
                miny = min(miny, y0); maxy = max(maxy, y1)
        else:
            for y in range(height):
                row = buf[y * width * 3:(y + 1) * width * 3]
                for x in range(width):
                    o = x * 3
                    if (abs(row[o] - bg_r) > 6 or
                            abs(row[o + 1] - bg_g) > 6 or
                            abs(row[o + 2] - bg_b) > 6):
                        minx = min(minx, x); maxx = max(maxx, x)
                        miny = min(miny, y); maxy = max(maxy, y)
    proc.stdout.close()
    err = proc.stderr.read() if proc.stderr else b""
    proc.wait()
    if proc.returncode != 0:
        sys.stderr.write(err.decode(errors="replace"))
        raise SystemExit("ffmpeg failed while measuring bbox")

    if count == 0:
        raise SystemExit("could not decode frames for bbox measurement")
    if maxx < 0:
        return 0, 0, width, height, count
    return minx, miny, maxx - minx + 1, maxy - miny + 1, count


def round_bbox(x: int, y: int, w: int, h: int, cw: int, ch: int) -> tuple[int, int, int, int]:
    """Even offsets, multiples of MCU, clamped inside the canvas."""
    x &= ~1
    y &= ~1
    w = max(MCU, (w + MCU - 1) & ~(MCU - 1))
    h = max(MCU, (h + MCU - 1) & ~(MCU - 1))
    if x + w > cw:
        w = cw - x
    if y + h > ch:
        h = ch - y
    w = max(MCU, w & ~(MCU - 1))
    h = max(MCU, h & ~(MCU - 1))
    if x + w > cw:
        x = max(0, cw - w)
    if y + h > ch:
        y = max(0, ch - h)
    return x, y, w, h


# --------------------------------------------------------------------------- #
# JPEG stream framing (shared logic with components/masko_core)
# --------------------------------------------------------------------------- #

def index_mjpeg(data: bytes) -> list[tuple[int, int]]:
    """Locate every complete baseline JPEG in a concatenated stream.

    A full marker walk, not a naive SOI/EOI search, so a stray 0xFF inside entropy-coded
    data cannot be mistaken for a marker.
    """
    frames: list[tuple[int, int]] = []
    i, n = 0, len(data)
    while i + 1 < n:
        if data[i] != 0xFF or data[i + 1] != 0xD8:
            i += 1
            continue
        start = i
        closed = False
        i += 2
        while i + 1 < n:
            if data[i] != 0xFF:
                i += 1
                continue
            m = data[i + 1]
            if m == 0xFF:
                i += 1
                continue
            if m == 0x00 or 0xD0 <= m <= 0xD7:
                i += 2
                continue
            if m == 0xD9:
                i += 2
                frames.append((start, i))
                closed = True
                break
            if i + 3 >= n:
                break
            seg = (data[i + 2] << 8) | data[i + 3]
            if seg < 2 or i + 2 + seg > n:
                break
            i += 2 + seg
        if not closed:
            break
    return frames


# --------------------------------------------------------------------------- #
# pack assembly
# --------------------------------------------------------------------------- #

def build(args) -> int:
    src = args.input
    if not os.path.exists(src):
        raise SystemExit(f"no such file: {src}")

    size = args.size
    bg_rgb = parse_colour(args.bg)
    bg_hex = "0x%02x%02x%02x" % bg_rgb
    alpha = has_real_alpha(src)

    cw, ch = probe_resolution(src)
    print(f"source        : {src}")
    print(f"  resolution  : {cw}x{ch}")
    print(f"  real alpha  : {'yes (compositing onto ' + bg_hex + ')' if alpha else 'no'}")

    if size <= 0:
        size = max(cw, ch)
    # Bbox dimensions must be MCU multiples or 4:2:0 chroma straddles a crop
    # edge. An opaque clip has no measured bbox, so the canvas *is* the bbox:
    # snap the canvas down to a multiple of MCU rather than emitting a pack that
    # fails its own verify. 466 -> 464, leaving a 1px border.
    size = max(MCU, (size // MCU) * MCU)

    crop = None
    if args.crop and alpha:
        bx, by, bw, bh, nf = measure_bbox(src, size, size, args.fps, bg_rgb, alpha)
        rx, ry, rw, rh = round_bbox(bx, by, bw, bh, size, size)
        crop = (rx, ry, rw, rh)
        print(f"  bbox        : union {bw}x{bh}+{bx}+{by} -> {rw}x{rh}+{rx}+{ry}"
              f"  (MCU-aligned, {100*rw*rh/(size*size):.0f}% of canvas)")
    elif args.crop:
        print("  bbox        : skipped, source is opaque")

    # --- encode to a concatenated MJPEG stream -------------------------------- #
    cmd = ["ffmpeg", "-hide_banner", "-y", "-v", "error"]
    if alpha:
        dur = _duration(src)
        cmd += ["-f", "lavfi", "-t", str(dur),
                "-i", f"color=c={bg_hex}:s={size}x{size}:r={args.fps}"]
    cmd += ["-i", src, "-an", "-map_metadata", "-1"]

    chain = f"[1:v]fps={args.fps},scale={size}:{size}:flags=lanczos,setsar=1"
    if crop:
        x, y, w, h = crop
        chain += f",crop={w}:{h}:{x}:{y}"
    if alpha:
        chain += ",format=rgba[fg]"
        graph = chain + ";[0:v][fg]overlay=x=0:y=0,format=yuvj420p[out]"
        cmd += ["-filter_complex", graph, "-map", "[out]"]
    else:
        cmd += ["-vf", chain + ",format=yuvj420p"]

    cmd += ["-c:v", "mjpeg", "-q:v", str(args.quality), "-f", "mjpeg", "-"]
    mjpeg = subprocess.run(cmd, capture_output=True)
    if mjpeg.returncode != 0 or not mjpeg.stdout:
        sys.stderr.write(mjpeg.stderr.decode(errors="replace"))
        raise SystemExit("ffmpeg encode failed")

    frames = index_mjpeg(mjpeg.stdout)
    if len(frames) < 2:
        raise SystemExit(f"only {len(frames)} frame(s) encoded")
    frame_count = len(frames)

    # --- assemble ------------------------------------------------------------ #
    header_size = HEADER_SIZE
    index_size = frame_count * 8
    data_offset = (header_size + index_size + PAYLOAD_ALIGN - 1) // PAYLOAD_ALIGN * PAYLOAD_ALIGN
    index_offset = header_size

    payload = bytearray()
    entries = bytearray()
    for (s, e) in frames:
        blob = mjpeg.stdout[s:e]
        # Pad FIRST, then record where the blob actually landed. Recording the
        # pre-pad offset makes every entry after the first point into the padding.
        payload += bytes((-(data_offset + len(payload))) % PAYLOAD_ALIGN)
        here = data_offset + len(payload)
        entries += struct.pack("<II", here, len(blob))
        payload += blob

    file_size = data_offset + len(payload)
    # CRC covers index_offset..EOF, i.e. the index, the gap up to data_offset and
    # the payload. A verifier hashes that same span, so both must agree.
    index_region = bytes(entries) + bytes(data_offset - (header_size + index_size))
    bx, by, bw, bh = crop if crop else (0, 0, size, size)

    flags = FLAG_LOOP if args.loop else 0
    if alpha:
        flags |= FLAG_ALPHA

    payload_crc = binascii.crc32(index_region + bytes(payload)) & 0xFFFFFFFF

    v = ((bg_rgb[0] >> 3) << 11) | ((bg_rgb[1] >> 2) << 5) | (bg_rgb[2] >> 3)
    bg_rgb565 = ((v & 0xFF) << 8) | (v >> 8)   # stored big-endian, matching the panel

    header = bytearray(HEADER_SIZE)
    struct.pack_into("<4sHH", header, 0, MAGIC, FORMAT_VERSION, HEADER_SIZE)
    header[8] = CODEC_JPEG
    header[9] = flags
    header[10] = SUBSAMPLING_420
    header[11] = 0
    struct.pack_into("<HHHHHHHH", header, 12,
                     size, size, bx, by, bw, bh, args.fps, 1)
    struct.pack_into("<III", header, 28, frame_count, index_offset, data_offset)
    struct.pack_into("<I", header, 40, file_size)
    struct.pack_into("<HH", header, 44, bg_rgb565, 0)
    struct.pack_into("<I", header, 48, payload_crc)
    header_crc = binascii.crc32(bytes(header[:52])) & 0xFFFFFFFF
    struct.pack_into("<I", header, 52, header_crc)

    out = args.out or (os.path.splitext(src)[0] + ".mkpk")
    with open(out, "wb") as f:
        f.write(header)
        f.write(entries)
        f.write(b"\x00" * (data_offset - (header_size + index_size)))
        f.write(payload)

    sizes = [e - s for (s, e) in frames]
    avg, mx = sum(sizes) / len(sizes), max(sizes)
    print(f"pack          : {out}")
    print(f"  codec       : JPEG baseline 4:2:0, q={args.quality}")
    print(f"  frames      : {frame_count} @ {args.fps} fps")
    print(f"  frame bytes : avg {avg:.0f}, max {mx}")
    print(f"  total       : {file_size:,} B ({file_size/1048576:.2f} MB)")
    est_decode = (bw * bh) / (7.4e6) * 1000
    est_push = (bw * bh * 2) / (16.4e6) * 1000
    print(f"  est. device : decode ~{est_decode:.1f} ms, push ~{est_push:.1f} ms "
          f"-> serial ~{1e3/(est_decode+est_push):.0f} fps, "
          f"overlapped ~{1e3/max(est_decode,est_push):.0f} fps")
    return 0


def read_header(data: bytes) -> dict:
    if len(data) < HEADER_SIZE or data[:4] != MAGIC:
        raise SystemExit("not a .mkpk file (bad magic)")
    ver, hsz = struct.unpack_from("<HH", data, 4)
    if ver != FORMAT_VERSION:
        raise SystemExit(f"unsupported format_version {ver}")
    if hsz != HEADER_SIZE:
        raise SystemExit(f"unexpected header_size {hsz}")
    (cw, ch, bx, by, bw, bh, fn, fd) = struct.unpack_from("<8H", data, 12)
    (fc, io, do, fs) = struct.unpack_from("<4I", data, 28)
    bg, _ = struct.unpack_from("<HH", data, 44)
    (pcrc, hcrc) = struct.unpack_from("<2I", data, 48)
    return dict(version=ver, header_size=hsz, codec=data[8], flags=data[9],
                subsampling=data[10], canvas_w=cw, canvas_h=ch,
                bbox_x=bx, bbox_y=by, bbox_w=bw, bbox_h=bh,
                fps_num=fn, fps_den=fd, frame_count=fc, index_offset=io,
                data_offset=do, file_size=fs, bg_rgb565=bg,
                payload_crc=pcrc, header_crc=hcrc,
                header_crc_ok=(binascii.crc32(data[:52]) & 0xFFFFFFFF) == hcrc)


def cmd_inspect(args) -> int:
    data = open(args.pack, "rb").read()
    h = read_header(data)
    print(f"{args.pack}  ({len(data):,} bytes)")
    for k in ("version", "codec", "flags", "subsampling", "canvas_w", "canvas_h",
              "bbox_x", "bbox_y", "bbox_w", "bbox_h", "fps_num", "fps_den",
              "frame_count", "data_offset", "file_size", "header_crc_ok"):
        print(f"  {k:<14}: {h[k]}")
    print(f"  decode pixels: {h['bbox_w']*h['bbox_h']:,}")
    return 0


def cmd_verify(args) -> int:
    data = open(args.pack, "rb").read()
    h = read_header(data)
    ok = True

    def check(cond, msg):
        nonlocal ok
        print(f"  [{'PASS' if cond else 'FAIL'}] {msg}")
        ok = ok and cond

    print(f"{args.pack}")
    check(h["header_crc_ok"], "header CRC")
    check(h["bbox_x"] % 2 == 0 and h["bbox_y"] % 2 == 0, "bbox origin is even")
    check(h["bbox_w"] % MCU == 0 and h["bbox_h"] % MCU == 0, "bbox size is MCU-aligned")
    check(h["bbox_x"] + h["bbox_w"] <= h["canvas_w"], "bbox fits horizontally")
    check(h["bbox_y"] + h["bbox_h"] <= h["canvas_h"], "bbox fits vertically")

    entries = [struct.unpack_from("<II", data, h["index_offset"] + i * 8)
               for i in range(h["frame_count"])]
    check(all(off % PAYLOAD_ALIGN == 0 for off, _ in entries),
          "every frame is 512-byte aligned")
    bad = 0
    for off, ln in entries:
        if data[off:off + 2] != b"\xff\xd8" or data[off + ln - 2:off + ln] != b"\xff\xd9":
            bad += 1
    check(bad == 0, f"all {len(entries)} frames are FFD8..FFD9")

    crc = binascii.crc32(data[h["index_offset"]:]) & 0xFFFFFFFF
    check(crc == h["payload_crc"], "payload CRC")

    print("OK" if ok else "FAILED")
    return 0 if ok else 1


def cmd_preview(args) -> int:
    data = open(args.pack, "rb").read()
    h = read_header(data)
    w, hh = h["bbox_w"], h["bbox_h"]
    out = args.out or (os.path.splitext(args.pack)[0] + ".mp4")
    proc = subprocess.Popen(
        ["ffmpeg", "-hide_banner", "-y", "-v", "error",
         "-f", "mjpeg", "-i", "pipe:0",
         "-vf", f"scale={w}:{hh}",
         "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
         "-movflags", "+faststart", out],
        stdin=subprocess.PIPE, stderr=subprocess.PIPE)
    _, err = proc.communicate(input=data[h["data_offset"]:])
    if proc.returncode != 0:
        sys.stderr.write(err.decode(errors="replace"))
        raise SystemExit("preview encode failed")
    print(f"wrote {out}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    b = sub.add_parser("build", help="convert a source clip into a .mkpk pack")
    b.add_argument("input")
    b.add_argument("-o", "--out")
    b.add_argument("--fps", type=int, default=24)
    b.add_argument("--quality", type=int, default=9, help="ffmpeg -q:v, 1=best .. 31=worst")
    b.add_argument("--bg", default="000000", help="background colour for alpha sources")
    b.add_argument("--size", type=int, default=466,
                   help="canvas size; rounded down to a multiple of the 16px MCU")
    b.add_argument("--crop", action="store_true", help="crop to the union bbox")
    b.add_argument("--loop", action="store_true", default=True)
    b.set_defaults(fn=build)

    i = sub.add_parser("inspect", help="dump the header")
    i.add_argument("pack")
    i.set_defaults(fn=cmd_inspect)

    v = sub.add_parser("verify", help="structural checks")
    v.add_argument("pack")
    v.set_defaults(fn=cmd_verify)

    p = sub.add_parser("preview", help="render a pack to an MP4")
    p.add_argument("pack")
    p.add_argument("-o", "--out")
    p.set_defaults(fn=cmd_preview)

    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())