"""Writes icons/icon.ico: a gold diamond on dark ground (PNG inside ICO, stdlib only)."""
import struct, zlib
from pathlib import Path

S = 64
rows = b""
for y in range(S):
    rows += b"\0"
    for x in range(S):
        d = abs(x - S / 2 + 0.5) + abs(y - S / 2 + 0.5)
        rows += bytes((223, 207, 153, 255) if d < 22 else (20, 17, 14, 255) if d < 30 else (0, 0, 0, 0))
chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
ico = struct.pack("<HHH", 0, 1, 1) + struct.pack("<BBBBHHII", S, S, 0, 0, 1, 32, len(png), 22) + png
Path(__file__).resolve().parent.parent.joinpath("icons", "icon.ico").write_bytes(ico)
