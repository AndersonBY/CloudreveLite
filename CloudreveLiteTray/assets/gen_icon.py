#!/usr/bin/env python3
"""Generate cloud.ico (PNG-entry ICO) and MSIX logo PNGs for CloudreveLiteTray.

Pure stdlib: renders a blue rounded-square with a white cloud via signed
distance fields + 3x3 supersampling, writes PNGs (zlib) and packs them
into an ICO container.
"""
import struct, zlib, os

OUT = os.path.dirname(os.path.abspath(__file__))

# ---- PNG writer -----------------------------------------------------------

def png_chunk(tag: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

def write_png(path: str, rgba: bytearray, w: int, h: int):
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)  # filter: none
        raw.extend(rgba[y * stride:(y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    blob = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr) + png_chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + png_chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(blob)

# ---- shape helpers (normalized coords 0..1) --------------------------------

def sd_circle(px, py, cx, cy, r):
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5 - r

def sd_round_rect(px, py, x0, y0, x1, y1, r):
    # rectangle from (x0,y0) to (x1,y1) with corner radius r, all normalized
    qx = abs(px - (x0 + x1) / 2) - (x1 - x0) / 2 + r
    qy = abs(py - (y0 + y1) / 2) - (y1 - y0) / 2 + r
    return min(max(qx, qy), 0.0) + (max(qx, 0.0) ** 2 + max(qy, 0.0) ** 2) ** 0.5 - r

def sd_roundedbox(px, py, bx, by, hw, hh, r):
    # box centered? no: use corner-point formulation
    qx = abs(px - bx) - hw + r
    qy = abs(py - by) - hh + r
    return min(max(qx, qy), 0.0) + (max(qx, 0.0) ** 2 + max(qy, 0.0) ** 2) ** 0.5 - r

def lerp(a, b, t):
    return int(a + (b - a) * t)

CLOUD = [
    # (cx, cy, r) circles
    (0.345, 0.575, 0.150),
    (0.500, 0.500, 0.195),
    (0.650, 0.565, 0.135),
]
CLOUD_BASE = (0.500, 0.640, 0.265, 0.085, 0.075)  # bx, by, half-w, half-h, corner r

BG_TOP = (61, 139, 253)   # #3D8BFD
BG_BOT = (24, 98, 219)    # #1862DB

def coverage(sdf, aa):
    return max(0.0, min(1.0, 0.5 - sdf * aa))

def render(size: int) -> tuple[bytearray, int, int]:
    ss = 3  # supersample factor
    w = h = size
    buf = bytearray(w * h * 4)
    aa = size  # 1px in normalized units == size px
    for py in range(h):
        for px in range(w):
            acc = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    u = (px + (sx + 0.5) / ss) / w
                    v = (py + (sy + 0.5) / ss) / h
                    sdf_bg = sd_roundedbox(u, v, 0.5, 0.5, 0.485, 0.485, 0.22)
                    bg = coverage(sdf_bg, aa)
                    csdf = min([sd_circle(u, v, c[0], c[1], c[2]) for c in CLOUD] + [sd_roundedbox(u, v, *CLOUD_BASE)])
                    cl = coverage(csdf, aa)
                    acc += bg * (1.0 - cl)  # cloud punches out of bg
            alpha_bg = acc / (ss * ss)
            if alpha_bg <= 0.0:
                continue
            t = py / h
            r, g, b = lerp(BG_TOP[0], BG_BOT[0], t), lerp(BG_TOP[1], BG_BOT[1], t), lerp(BG_TOP[2], BG_BOT[2], t)
            i = (py * w + px) * 4
            buf[i] = r
            buf[i + 1] = g
            buf[i + 2] = b
            buf[i + 3] = int(alpha_bg * 255 + 0.5)
    return buf, w, h

def render_png_bytes(size: int) -> bytes:
    import io
    buf, w, h = render(size)
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)
        raw.extend(buf[y * stride:(y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr) + png_chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + png_chunk(b"IEND", b"")

def build_ico(path: str, sizes):
    pngs = [(s, render_png_bytes(s)) for s in sizes]
    count = len(pngs)
    header = struct.pack("<HHH", 0, 1, count)
    entries = bytearray()
    data = bytearray()
    offset = 6 + 16 * count
    for s, blob in pngs:
        entries += struct.pack("<BBBBHHII", 0 if s >= 256 else s, 0 if s >= 256 else s, 0, 0, 1, 32, len(blob), offset)
        data += blob
        offset += len(blob)
    with open(path, "wb") as f:
        f.write(header + bytes(entries) + bytes(data))

if __name__ == "__main__":
    build_ico(os.path.join(OUT, "cloud.ico"), [16, 24, 32, 48, 64, 256])
    write_png(os.path.join(OUT, "Square150x150Logo.png"), *render(150))
    write_png(os.path.join(OUT, "Square44x44Logo.png"), *render(44))
    print("icons written to", OUT)
