#!/usr/bin/env python3
import argparse
import math
import os
import struct
import zlib


def png_bytes(width, height):
    rows = []
    for y in range(height):
        row = bytearray()
        for x in range(width):
            row.extend(pixel(width, height, x, y))
        rows.append(b"\x00" + bytes(row))
    raw = b"".join(rows)
    chunks = [
        chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)),
        chunk(b"IDAT", zlib.compress(raw, 9)),
        chunk(b"IEND", b""),
    ]
    return b"\x89PNG\r\n\x1a\n" + b"".join(chunks)


def chunk(kind, payload):
    crc = zlib.crc32(kind + payload) & 0xFFFFFFFF
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)


def pixel(width, height, x, y):
    scale = max(width, height)
    samples = 3 if scale <= 64 else 2
    rgba = [0.0, 0.0, 0.0, 0.0]
    for sy in range(samples):
        for sx in range(samples):
            px = (x + (sx + 0.5) / samples) / width
            py = (y + (sy + 0.5) / samples) / height
            c = sample(px, py)
            a = c[3]
            rgba[0] += c[0] * a
            rgba[1] += c[1] * a
            rgba[2] += c[2] * a
            rgba[3] += a
    n = samples * samples
    a = rgba[3] / n
    if a > 0:
        rgb = [rgba[i] / rgba[3] for i in range(3)]
    else:
        rgb = [0, 0, 0]
    return (
        int(max(0, min(255, round(rgb[0])))),
        int(max(0, min(255, round(rgb[1])))),
        int(max(0, min(255, round(rgb[2])))),
        int(max(0, min(255, round(a * 255)))),
    )


def sample(x, y):
    # Blue glass app tile.
    bg_mask = rounded_rect_fill(x, y, 0.08, 0.08, 0.84, 0.84, 0.19)
    if bg_mask <= 0:
        return (0, 0, 0, 0)

    top = (28, 151, 255)
    bottom = (17, 89, 224)
    t = min(1.0, max(0.0, y))
    bg = tuple(top[i] * (1 - t) + bottom[i] * t for i in range(3))
    glow = max(0.0, 1.0 - math.hypot((x - 0.35) / 0.55, (y - 0.18) / 0.35))
    bg = tuple(min(255, bg[i] + glow * 44) for i in range(3))
    out = [bg[0], bg[1], bg[2], bg_mask]

    # Edge highlight and inner glass sheen.
    edge = rounded_rect_stroke(x, y, 0.08, 0.08, 0.84, 0.84, 0.19, 0.018)
    out = over(out, (255, 255, 255, edge * 0.55))
    sheen = max(0.0, 1.0 - y / 0.45) * rounded_rect_fill(x, y, 0.11, 0.10, 0.78, 0.35, 0.15)
    out = over(out, (255, 255, 255, sheen * 0.20))

    # Clipboard glyph.
    white = (255, 255, 255)
    back = rounded_rect_stroke(x, y, 0.28, 0.20, 0.39, 0.58, 0.10, 0.045)
    front = rounded_rect_stroke(x, y, 0.39, 0.29, 0.39, 0.58, 0.10, 0.045)
    line1 = line_stroke(x, y, 0.46, 0.48, 0.66, 0.48, 0.045)
    line2 = line_stroke(x, y, 0.46, 0.62, 0.66, 0.62, 0.045)
    line3 = line_stroke(x, y, 0.50, 0.75, 0.62, 0.75, 0.045)
    glyph = max(back * 0.72, front, line1, line2, line3)
    out = over(out, (*white, glyph * 0.96))
    return tuple(out)


def over(dst, src):
    sr, sg, sb, sa = src
    dr, dg, db, da = dst
    out_a = sa + da * (1 - sa)
    if out_a <= 0:
        return [0, 0, 0, 0]
    return [
        (sr * sa + dr * da * (1 - sa)) / out_a,
        (sg * sa + dg * da * (1 - sa)) / out_a,
        (sb * sa + db * da * (1 - sa)) / out_a,
        out_a,
    ]


def rounded_rect_fill(px, py, x, y, w, h, r):
    d = rounded_rect_distance(px, py, x, y, w, h, r)
    return smooth(-d, 0.006)


def rounded_rect_stroke(px, py, x, y, w, h, r, stroke):
    d = abs(rounded_rect_distance(px, py, x, y, w, h, r)) - stroke / 2
    return smooth(-d, 0.006)


def rounded_rect_distance(px, py, x, y, w, h, r):
    cx = abs(px - (x + w / 2)) - (w / 2 - r)
    cy = abs(py - (y + h / 2)) - (h / 2 - r)
    ox = max(cx, 0)
    oy = max(cy, 0)
    return math.hypot(ox, oy) + min(max(cx, cy), 0) - r


def line_stroke(px, py, x1, y1, x2, y2, stroke):
    vx = x2 - x1
    vy = y2 - y1
    length = vx * vx + vy * vy
    t = ((px - x1) * vx + (py - y1) * vy) / length
    t = max(0, min(1, t))
    dx = px - (x1 + vx * t)
    dy = py - (y1 + vy * t)
    d = math.hypot(dx, dy) - stroke / 2
    return smooth(-d, 0.006)


def smooth(value, width):
    return max(0.0, min(1.0, 0.5 + value / width))


def write_png(path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(png_bytes(size, size))


def write_iconset(path):
    specs = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ]
    os.makedirs(path, exist_ok=True)
    for name, size in specs:
        write_png(os.path.join(path, name), size)


def write_ico(path):
    sizes = [16, 32, 48, 64, 128, 256]
    images = [png_bytes(size, size) for size in sizes]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + len(images) * 16
    entries = []
    for size, img in zip(sizes, images):
        dim = 0 if size == 256 else size
        entries.append(
            struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(img), offset)
        )
        offset += len(img)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(header + b"".join(entries) + b"".join(images))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--iconset")
    parser.add_argument("--ico")
    args = parser.parse_args()
    if not args.iconset and not args.ico:
        parser.error("provide --iconset and/or --ico")
    if args.iconset:
        write_iconset(args.iconset)
    if args.ico:
        write_ico(args.ico)


if __name__ == "__main__":
    main()
