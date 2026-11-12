"""Writes icons/icon.ico and the PNG sizes Tauri wants: a gold compass diamond
on dark stone, in the muted gold palette of Eni-PoE2-Companion (stdlib only)."""

import struct
import zlib
from pathlib import Path

ICONS = Path(__file__).resolve().parent.parent / "icons"


def render(size: int) -> bytes:
    """RGBA rows, top-down, with a soft diamond + inner ring."""
    cx = cy = size / 2 - 0.5
    outer = size * 0.40
    inner = size * 0.17
    rows = bytearray()
    for y in range(size):
        rows.append(0)  # PNG filter: none
        for x in range(size):
            d = abs(x - cx) + abs(y - cy)          # diamond distance
            e = max(abs(x - cx), abs(y - cy))      # square distance for the plate
            if d < inner:
                px = (245, 238, 214, 255)          # bright core
            elif d < outer:
                px = (212, 184, 122, 255)          # muted gold
            elif e < size * 0.46:
                px = (24, 21, 16, 255)             # dark plate
            else:
                px = (0, 0, 0, 0)                  # transparent
            rows += bytes(px)
    return bytes(rows)


def png(size: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    raw = render(size)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def write_ico(path: Path, sizes: list[int]) -> None:
    images = [(s, png(s)) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(blobs))
        blobs += data
    path.write_bytes(header + entries + blobs)


def main() -> None:
    ICONS.mkdir(exist_ok=True)
    write_ico(ICONS / "icon.ico", [16, 24, 32, 48, 64, 128, 256])
    for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"), (512, "icon.png")]:
        (ICONS / name).write_bytes(png(size))
    print("icons written:", ICONS)


if __name__ == "__main__":
    main()
