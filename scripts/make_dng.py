"""Writes a small synthetic DNG for testing Camera Raw: a Bayer (RGGB)
mosaic of a picture whose colors are known (red left, blue right, a green
band across the top, a gray square in the middle), in a "camera" whose
color space is linear sRGB, so a correct decode gives those colors back.

    python3 scripts/make_dng.py <out.dng> [orientation]
"""
import struct
import sys

out = sys.argv[1]
orientation = int(sys.argv[2]) if len(sys.argv) > 2 else 1
W, H = 96, 64


def color(x, y):
    """Linear light, 0 to 1."""
    if 24 <= x < 72 and 20 <= y < 44:
        return (0.18, 0.18, 0.18)
    if y < 12:
        return (0.05, 0.6, 0.05)
    return (0.7, 0.05, 0.05) if x < W // 2 else (0.05, 0.05, 0.7)


pixels = bytearray()
for y in range(H):
    for x in range(W):
        r, g, b = color(x, y)
        # RGGB: red at even/even, blue at odd/odd, green elsewhere.
        v = r if (x % 2, y % 2) == (0, 0) else b if (x % 2, y % 2) == (1, 1) else g
        pixels += struct.pack("<H", round(v * 65535))

XYZ_TO_SRGB = [3.2406, -1.5372, -0.4986, -0.9689, 1.8758, 0.0415, 0.0557, -0.2040, 1.0570]
SHORT, LONG, RATIONAL, SRATIONAL, BYTE, ASCII = 3, 4, 5, 10, 1, 2
entries = {}


def tag(t, kind, values):
    entries[t] = (kind, values)


tag(254, LONG, [0])
tag(256, LONG, [W])
tag(257, LONG, [H])
tag(258, SHORT, [16])
tag(259, SHORT, [1])
tag(262, SHORT, [32803])
tag(271, ASCII, b"grace9000\0")
tag(272, ASCII, b"Synthetic\0")
tag(273, LONG, [0])  # filled in below
tag(274, SHORT, [orientation])
tag(277, SHORT, [1])
tag(278, LONG, [H])
tag(279, LONG, [len(pixels)])
tag(284, SHORT, [1])
tag(33421, SHORT, [2, 2])
tag(33422, BYTE, [0, 1, 1, 2])
tag(50706, BYTE, [1, 4, 0, 0])
tag(50708, ASCII, b"grace9000 Synthetic\0")
tag(50714, LONG, [0])
tag(50717, LONG, [65535])
tag(50721, SRATIONAL, [(round(v * 10000), 10000) for v in XYZ_TO_SRGB])
tag(50728, RATIONAL, [(1, 1)] * 3)
tag(50778, SHORT, [21])

SIZES = {SHORT: 2, LONG: 4, RATIONAL: 8, SRATIONAL: 8, BYTE: 1, ASCII: 1}


def pack(kind, values):
    if kind == ASCII:
        return values
    if kind in (RATIONAL, SRATIONAL):
        f = "<II" if kind == RATIONAL else "<ii"
        return b"".join(struct.pack(f, *v) for v in values)
    f = {SHORT: "<H", LONG: "<I", BYTE: "<B"}[kind]
    return b"".join(struct.pack(f, v) for v in values)


ifd_at = 8
count = len(entries)
data_at = ifd_at + 2 + count * 12 + 4
extra = bytearray()
pix_at = None
# Two passes: first find where the pixels go, then write.
for final in (False, True):
    extra = bytearray()
    ifd = struct.pack("<H", count)
    for t in sorted(entries):
        kind, values = entries[t]
        if t == 273 and final:
            values = [pix_at]
        raw = pack(kind, values)
        n = len(values)
        if len(raw) <= 4:
            ifd += struct.pack("<HHI", t, kind, n) + raw.ljust(4, b"\0")
        else:
            off = data_at + len(extra)
            ifd += struct.pack("<HHII", t, kind, n, off)
            extra += raw
            if len(extra) % 2:
                extra += b"\0"
    ifd += struct.pack("<I", 0)
    pix_at = data_at + len(extra)

with open(out, "wb") as f:
    f.write(b"II*\0" + struct.pack("<I", ifd_at) + ifd + extra + pixels)
print(out)
