#!/usr/bin/env python3
"""Draw enoch's app icon: the menu bar clock on a dark rounded square.

Writes a 1024x1024 PNG (standard library only). CI turns it into the
.icns the app bundle uses (see build-app.sh).

    python3 macos/make_icon.py macos/icon.png
"""

import math
import struct
import sys
import zlib

SIZE = 1024
# Apple's icon grid: the shape is 824px wide, centred, on a 1024px canvas.
BODY = 824
# The window's accent color, and a dark slate background.
ACCENT = (80, 200, 220)
TOP, BOTTOM = (40, 52, 66), (20, 26, 33)


def clamp(v):
    return max(0.0, min(1.0, v))


def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = clamp(((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def over(dst, src, a):
    return tuple(d + (s - d) * a for d, s in zip(dst, src))


def pixel(x, y):
    """RGBA at pixel centre (x, y), with coverage-based anti-aliasing."""
    c = SIZE / 2
    px, py = x + 0.5 - c, y + 0.5 - c
    # The rounded square: a superellipse, |x|^5 + |y|^5 = r^5.
    r = BODY / 2
    n = 5.0
    f = (abs(px) ** n + abs(py) ** n) ** (1 / n) - r
    shape = clamp(0.5 - f)
    if shape == 0:
        return (0, 0, 0, 0)
    t = (py + r) / (2 * r)
    rgb = over(TOP, BOTTOM, clamp(t))

    # The clock, as in the menu bar: a ring, a faint face, hands at 12 and 2.
    s = BODY * 0.78
    d = math.hypot(px, py)
    r_out = s * 0.45
    ring = s * 0.085
    hand = s * 0.08
    ring_a = clamp(ring / 2 + 0.5 - abs(d - (r_out - ring / 2)))
    face_a = clamp(r_out - ring + 0.5 - d) * 0.14
    hands = [(0.0, -s * 0.27), (s * 0.18, -s * 0.10)]
    hand_a = max(clamp(hand / 2 + 0.5 - seg_dist(px, py, 0, 0, hx, hy)) for hx, hy in hands)
    hub_a = clamp(hand * 0.9 + 0.5 - d)
    rgb = over(rgb, ACCENT, face_a)
    rgb = over(rgb, ACCENT, max(ring_a, hand_a, hub_a))
    return tuple(round(v) for v in rgb) + (round(shape * 255),)


def png(width, height, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            row.extend(pixel(x, y))
        rows.append(row)
    with open(out, "wb") as f:
        f.write(png(SIZE, SIZE, rows))


if __name__ == "__main__":
    main()
