"""Generate Saywell's app icon (PNG + ICO) with no dependencies.

Design: dark slate rounded tile with five amber rounded voice bars.
Usage: python scripts/gen-icon.py <out_dir>
"""
import struct
import sys
import zlib
from pathlib import Path

BG = (30, 36, 48)
FG = (242, 165, 65)
BARS = [0.30, 0.55, 0.80, 0.55, 0.30]  # bar heights, as a fraction of tile size


def coverage(x: float, y: float) -> tuple[float, float]:
    """Return (tile alpha, bar alpha) at unit coords in [0, 1)."""
    r = 0.22
    cx, cy = min(max(x, r), 1 - r), min(max(y, r), 1 - r)
    tile = 1.0 if (x - cx) ** 2 + (y - cy) ** 2 <= r * r else 0.0
    bw, gap = 0.085, 0.055
    total = len(BARS) * bw + (len(BARS) - 1) * gap
    x0 = (1 - total) / 2
    for i, h in enumerate(BARS):
        bx = x0 + i * (bw + gap)
        if bx <= x <= bx + bw:
            top, bot = 0.5 - h / 2, 0.5 + h / 2
            rr = bw / 2
            mx = bx + rr
            yy = min(max(y, top + rr), bot - rr)
            if (x - mx) ** 2 + (y - yy) ** 2 <= rr * rr:
                return tile, 1.0
    return tile, 0.0


def render(size: int, ss: int = 4) -> bytes:
    rows = bytearray()
    for py in range(size):
        rows.append(0)
        for px in range(size):
            ta = ba = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    t, b = coverage((px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size)
                    ta += t
                    ba += b
            n = ss * ss
            ta, ba = ta / n, ba / n
            col = [round(BG[c] * (1 - ba) + FG[c] * ba) for c in range(3)]
            rows += bytes(col + [round(255 * max(ta, ba))])
    return bytes(rows)


def png(size: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(render(size), 9)) + chunk(b"IEND", b"")


def ico(images: dict[int, bytes]) -> bytes:
    head = struct.pack("<HHH", 0, 1, len(images))
    entries, blobs, offset = b"", b"", 6 + 16 * len(images)
    for size, data in images.items():
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return head + entries + blobs


if __name__ == "__main__":
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    big = png(512)
    (out / "icon.png").write_bytes(big)
    (out / "32x32.png").write_bytes(png(32))
    (out / "icon.ico").write_bytes(ico({256: png(256), 48: png(48), 32: png(32), 16: png(16)}))
    print("icons written to", out)
