#!/usr/bin/env python3
"""Bundle compatible .mkpk clips for the Waveshare transition demo."""
import argparse
import pathlib
import struct
import zlib

ALIGNMENT = 512


def dimensions(pack: bytes) -> tuple[int, int]:
    if len(pack) < 64 or pack[:4] != b"MKPK":
        raise ValueError("input is not an MKPK pack")
    # Header fields are little-endian; bbox width/height are u16 at 20/22.
    return int.from_bytes(pack[20:22], "little"), int.from_bytes(pack[22:24], "little")


def make_one_shot(pack: bytes) -> bytes:
    """Clear the MKPK loop bit and refresh the header CRC without touching frames."""
    if len(pack) < 64 or pack[:4] != b"MKPK":
        raise ValueError("input is not an MKPK pack")
    patched = bytearray(pack)
    patched[9] &= ~0x01
    struct.pack_into("<I", patched, 52, zlib.crc32(patched[:52]) & 0xFFFFFFFF)
    return bytes(patched)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("packs", type=pathlib.Path, nargs="+", metavar="PACK",
                        help="two or more packs, stored in the order supplied")
    parser.add_argument("--once", action="append", type=pathlib.Path, default=[],
                        metavar="PACK",
                        help="store this pack as one-shot (may be repeated)")
    args = parser.parse_args()
    if len(args.packs) < 2:
        parser.error("provide at least two packs")
    once = {path.resolve() for path in args.once}
    blobs = [make_one_shot(path.read_bytes()) if path.resolve() in once
             else path.read_bytes() for path in args.packs]
    size = dimensions(blobs[0])
    if any(dimensions(blob) != size for blob in blobs[1:]):
        raise SystemExit("all packs must have matching bbox dimensions")
    output = bytearray(blobs[0])
    for blob in blobs[1:]:
        output.extend(b"\xff" * ((-len(output)) % ALIGNMENT))
        output.extend(blob)
    args.output.write_bytes(output)
    print(f"wrote {len(blobs)} clips ({len(output)} bytes) to {args.output}")


if __name__ == "__main__":
    main()
