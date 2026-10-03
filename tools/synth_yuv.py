#!/usr/bin/env python3
"""Writes a deterministic synthetic I420 sequence to stdout, as the source
for tools/xvid-vectors.sh.

    synth_yuv.py WIDTH HEIGHT FRAMES [--interlaced]

The picture is a textured plane seen through a camera that pans, zooms and
turns slowly (global motion, for GMC), with a bright square crossing it at
a sub-sample speed (local motion, for quarter-sample search) and a little
pseudo-random grain (so no block is trivially flat). With --interlaced the
bottom field of each frame is sampled half a frame later than the top
field, so the two fields of a moving picture differ as a camera's do.
"""
import math
import sys


def texture(x, y):
    return (96.0 + 40.0 * math.sin(x * 0.11) + 30.0 * math.cos(y * 0.07)
            + 20.0 * math.sin((x + y) * 0.31) + 12.0 * math.sin(x * 0.53 - y * 0.29))


def grain(x, y, t):
    h = (x * 73856093) ^ (y * 19349663) ^ (t * 83492791)
    h = (h ^ (h >> 13)) * 0x5bd1e995 & 0xffffffff
    return ((h >> 24) & 7) - 3.5


def luma(w, h, t, x, y):
    zoom = 1.0 + 0.004 * t
    ang = 0.002 * t
    cx, cy = w / 2.0, h / 2.0
    dx, dy = x - cx, y - cy
    u = (dx * math.cos(ang) - dy * math.sin(ang)) / zoom + cx + 1.25 * t
    v = (dx * math.sin(ang) + dy * math.cos(ang)) / zoom + cy + 0.5 * t
    val = texture(u, v)
    sx, sy = w * 0.7 - 2.25 * t, h * 0.3 + 0.75 * t
    if abs(x - sx) < 12 and abs(y - sy) < 10:
        val = 225.0 - 4.0 * ((int(x) + int(y)) % 5)
    return val


def main():
    w, h, n = (int(a) for a in sys.argv[1:4])
    interlaced = "--interlaced" in sys.argv[4:]
    cw, ch = (w + 1) // 2, (h + 1) // 2
    out = sys.stdout.buffer
    for t in range(n):
        y_plane = bytearray(w * h)
        for r in range(h):
            tt = t + (0.5 if interlaced and r & 1 else 0.0)
            for c in range(w):
                v = luma(w, h, tt, c, r) + grain(c, r, t)
                y_plane[r * w + c] = max(0, min(255, int(v)))
        out.write(y_plane)
        for phase in (0.0, 1.3):
            p = bytearray(cw * ch)
            for r in range(ch):
                for c in range(cw):
                    x, y = c * 2 + 1.25 * t, r * 2 + 0.5 * t
                    v = 128 + 30 * math.sin(x * 0.05 + phase) + 20 * math.cos(y * 0.04)
                    p[r * cw + c] = max(0, min(255, int(v)))
            out.write(p)


main()
