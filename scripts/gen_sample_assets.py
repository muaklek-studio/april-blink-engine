#!/usr/bin/env python3
"""Generates the engine's sample assets (stdlib only, deterministic output).

    python3 scripts/gen_sample_assets.py            # writes into assets/models/ and assets/audio/

Outputs:
  totem.gltf + totem_albedo.png  Skinned 3-segment totem (3-joint chain) with two animation
                                 clips ("Sway", "Spin"). Buffer embedded as a base64 data URI,
                                 base color texture as an external PNG.
  crate.glb                      Binary glTF cube with base color + normal map PNGs embedded
                                 in the BIN chunk (exercises bufferView images, no tangents).
  audio/thud.wav                 Short low impact (crates landing).
  audio/hum.wav                  2 s seamless loop: the crystal's drone (positional).
  audio/wind.wav                 4 s seamless loop: filtered-noise night wind (ambience).

All content is generated here and released with the engine under its MIT license.
"""

import base64
import json
import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "assets" / "models"
AUDIO_OUT = OUT.parent / "audio"

# ----------------------------------------------------------------------------------------
# PNG
# ----------------------------------------------------------------------------------------


def png(width, height, pixel):
    """Encodes an RGBA8 PNG; `pixel(x, y)` returns (r, g, b, a)."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)  # filter: none
        for x in range(width):
            rows.extend(pixel(x, y))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )


def hash_noise(x, y, seed=0):
    """Deterministic value noise in [0, 1)."""
    n = (x * 374761393 + y * 668265263 + seed * 2147483647) & 0xFFFFFFFF
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    return ((n ^ (n >> 16)) & 0xFFFF) / 65536.0


def clamp8(v):
    return max(0, min(255, int(round(v))))


# ----------------------------------------------------------------------------------------
# Geometry helpers
# ----------------------------------------------------------------------------------------

# (normal, u axis, v axis); u x v = normal so quads wind counter-clockwise from outside.
FACES = [
    ((1, 0, 0), (0, 0, -1), (0, 1, 0)),
    ((-1, 0, 0), (0, 0, 1), (0, 1, 0)),
    ((0, 1, 0), (1, 0, 0), (0, 0, -1)),
    ((0, -1, 0), (1, 0, 0), (0, 0, 1)),
    ((0, 0, 1), (1, 0, 0), (0, 1, 0)),
    ((0, 0, -1), (-1, 0, 0), (0, 1, 0)),
]


def box(center, half, uv_rect=(0, 0, 1, 1)):
    """Returns (positions, normals, uvs, indices) of an axis-aligned box."""
    positions, normals, uvs, indices = [], [], [], []
    u0, v0, u1, v1 = uv_rect
    for n, u, v in FACES:
        base = len(positions)
        for su, sv in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
            p = [center[i] + (n[i] + u[i] * su + v[i] * sv) * half[i] for i in range(3)]
            positions.append(p)
            normals.append(list(n))
            uvs.append([u0 + (u1 - u0) * (su + 1) / 2, v0 + (v1 - v0) * (1 - sv) / 2])
        indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    return positions, normals, uvs, indices


class BufferBuilder:
    """Packs accessors into one buffer with 4-byte aligned views."""

    def __init__(self):
        self.data = bytearray()
        self.views = []
        self.accessors = []

    def _view(self, payload, target=None):
        while len(self.data) % 4:
            self.data.append(0)
        view = {"buffer": 0, "byteOffset": len(self.data), "byteLength": len(payload)}
        if target:
            view["target"] = target
        self.data.extend(payload)
        self.views.append(view)
        return len(self.views) - 1

    def raw_view(self, payload):
        return self._view(payload)

    def accessor(self, values, kind, component, target=None, bounds=False):
        fmt = {5126: "f", 5125: "I", 5123: "H"}[component]
        flat = [c for v in values for c in (v if isinstance(v, (list, tuple)) else [v])]
        view = self._view(struct.pack("<%d%s" % (len(flat), fmt), *flat), target)
        accessor = {"bufferView": view, "componentType": component, "count": len(values), "type": kind}
        if bounds:
            width = len(values[0])
            accessor["min"] = [min(v[i] for v in values) for i in range(width)]
            accessor["max"] = [max(v[i] for v in values) for i in range(width)]
        self.accessors.append(accessor)
        return len(self.accessors) - 1


FLOAT, UINT, USHORT = 5126, 5125, 5123
ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER = 34962, 34963

# ----------------------------------------------------------------------------------------
# Totem: skinned, animated, external texture
# ----------------------------------------------------------------------------------------

TOTEM_COLORS = [(38, 120, 128), (170, 54, 40), (204, 150, 52)]


def totem_albedo(x, y):
    band = min(2, y // 64)
    r, g, b = TOTEM_COLORS[2 - band]  # top segment first in image space
    local_y = y % 64
    # Carved rims at the top and bottom of every segment.
    if local_y < 4 or local_y > 59:
        r, g, b = (r * 0.45, g * 0.45, b * 0.45)
    # Eyes on every segment (two dark squares, repeated per face).
    face_x = x % 32
    if 18 <= local_y <= 28 and (6 <= face_x <= 12 or 20 <= face_x <= 26):
        r, g, b = (25, 20, 18)
    grain = 0.85 + 0.3 * hash_noise(x // 2, y, 7)
    return (clamp8(r * grain), clamp8(g * grain), clamp8(b * grain), 255)


def build_totem():
    buf = BufferBuilder()
    positions, normals, uvs, indices, joints, weights = [], [], [], [], [], []
    for segment in range(3):
        width = 0.7 - 0.1 * segment
        # Image rows: top segment at v=0.
        v0 = (2 - segment) / 3
        p, n, t, idx = box((0, segment + 0.5, 0), (width / 2, 0.48, width / 2), (0, v0, 1, v0 + 1 / 3))
        base = len(positions)
        positions += p
        normals += n
        uvs += t
        indices += [i + base for i in idx]
        joints += [[segment, 0, 0, 0]] * len(p)
        weights += [[1.0, 0.0, 0.0, 0.0]] * len(p)

    a_pos = buf.accessor(positions, "VEC3", FLOAT, ARRAY_BUFFER, bounds=True)
    a_nrm = buf.accessor(normals, "VEC3", FLOAT, ARRAY_BUFFER)
    a_uv = buf.accessor(uvs, "VEC2", FLOAT, ARRAY_BUFFER)
    a_jnt = buf.accessor(joints, "VEC4", USHORT, ARRAY_BUFFER)
    a_wgt = buf.accessor(weights, "VEC4", FLOAT, ARRAY_BUFFER)
    a_idx = buf.accessor(indices, "SCALAR", UINT, ELEMENT_ARRAY_BUFFER)

    # Inverse bind matrices: joints sit at y = 0, 1, 2 (column-major).
    ibm = []
    for j in range(3):
        ibm.append([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -float(j), 0, 1])
    a_ibm = buf.accessor(ibm, "MAT4", FLOAT)

    # "Sway": joints 1 and 2 rock around Z. "Spin": joint 0 turns around Y.
    sway_times = [0.0, 0.5, 1.0, 1.5, 2.0]
    sway_angles = [0.0, 0.3, 0.0, -0.3, 0.0]
    a_sway_t = buf.accessor(sway_times, "SCALAR", FLOAT, bounds=False)
    buf.accessors[a_sway_t]["min"] = [0.0]
    buf.accessors[a_sway_t]["max"] = [2.0]
    sway_q = [[0, 0, math.sin(a / 2), math.cos(a / 2)] for a in sway_angles]
    a_sway_q = buf.accessor(sway_q, "VEC4", FLOAT)

    spin_times = [0.0, 1.0, 2.0, 3.0, 4.0]
    a_spin_t = buf.accessor(spin_times, "SCALAR", FLOAT)
    buf.accessors[a_spin_t]["min"] = [0.0]
    buf.accessors[a_spin_t]["max"] = [4.0]
    spin_q = [[0, math.sin(k * math.pi / 4), 0, math.cos(k * math.pi / 4)] for k in range(5)]
    a_spin_q = buf.accessor(spin_q, "VEC4", FLOAT)

    gltf = {
        "asset": {"version": "2.0", "generator": "april-blink gen_sample_assets.py"},
        "scene": 0,
        "scenes": [{"name": "Totem", "nodes": [0]}],
        "nodes": [
            {"name": "Totem", "children": [1, 4]},
            {"name": "Joint0", "children": [2]},
            {"name": "Joint1", "translation": [0, 1, 0], "children": [3]},
            {"name": "Joint2", "translation": [0, 1, 0]},
            {"name": "TotemMesh", "mesh": 0, "skin": 0},
        ],
        "skins": [{"name": "Spine", "joints": [1, 2, 3], "inverseBindMatrices": a_ibm, "skeleton": 1}],
        "meshes": [
            {
                "name": "TotemMesh",
                "primitives": [
                    {
                        "attributes": {
                            "POSITION": a_pos,
                            "NORMAL": a_nrm,
                            "TEXCOORD_0": a_uv,
                            "JOINTS_0": a_jnt,
                            "WEIGHTS_0": a_wgt,
                        },
                        "indices": a_idx,
                        "material": 0,
                    }
                ],
            }
        ],
        "materials": [
            {
                "name": "PaintedWood",
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.75,
                },
            }
        ],
        "textures": [{"source": 0, "sampler": 0}],
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071}],
        "images": [{"uri": "totem_albedo.png"}],
        "animations": [
            {
                "name": "Sway",
                "samplers": [{"input": a_sway_t, "output": a_sway_q, "interpolation": "LINEAR"}],
                "channels": [
                    {"sampler": 0, "target": {"node": 2, "path": "rotation"}},
                    {"sampler": 0, "target": {"node": 3, "path": "rotation"}},
                ],
            },
            {
                "name": "Spin",
                "samplers": [{"input": a_spin_t, "output": a_spin_q, "interpolation": "LINEAR"}],
                "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}],
            },
        ],
        "bufferViews": buf.views,
        "accessors": buf.accessors,
        "buffers": [
            {
                "byteLength": len(buf.data),
                "uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(buf.data)).decode(),
            }
        ],
    }
    (OUT / "totem.gltf").write_text(json.dumps(gltf, indent=1) + "\n")
    (OUT / "totem_albedo.png").write_bytes(png(128, 192, totem_albedo))


# ----------------------------------------------------------------------------------------
# Crate: GLB with embedded base color + normal map
# ----------------------------------------------------------------------------------------

PLANKS = 4
SIZE = 128


def plank_height(x, y):
    """Height field: planks separated by grooves, framed by a raised border."""
    border = min(x, y, SIZE - 1 - x, SIZE - 1 - y)
    if border < 10:
        return 1.0
    plank_y = (y - 10) % ((SIZE - 20) // PLANKS)
    return 0.0 if plank_y < 2 else 0.6


def crate_albedo(x, y):
    h = plank_height(x, y)
    grain = 0.8 + 0.35 * hash_noise(x // 6, y, 3) * hash_noise(x // 2, y // 3, 11)
    if h >= 1.0:
        base = (120, 82, 46)
    elif h == 0.0:
        base = (40, 26, 16)
    else:
        base = (150, 104, 58)
    return tuple(clamp8(c * grain) for c in base) + (255,)


def crate_normal(x, y):
    strength = 4.0
    dx = plank_height(min(x + 1, SIZE - 1), y) - plank_height(max(x - 1, 0), y)
    dy = plank_height(x, min(y + 1, SIZE - 1)) - plank_height(x, max(y - 1, 0))
    # Image y grows downward while the tangent-space bitangent points up.
    n = (-dx * strength, dy * strength, 1.0)
    length = math.sqrt(sum(c * c for c in n))
    return tuple(clamp8((c / length * 0.5 + 0.5) * 255) for c in n) + (255,)


def build_crate():
    buf = BufferBuilder()
    p, n, t, idx = box((0, 0, 0), (0.5, 0.5, 0.5))
    a_pos = buf.accessor(p, "VEC3", FLOAT, ARRAY_BUFFER, bounds=True)
    a_nrm = buf.accessor(n, "VEC3", FLOAT, ARRAY_BUFFER)
    a_uv = buf.accessor(t, "VEC2", FLOAT, ARRAY_BUFFER)
    a_idx = buf.accessor(idx, "SCALAR", UINT, ELEMENT_ARRAY_BUFFER)
    v_albedo = buf.raw_view(png(SIZE, SIZE, crate_albedo))
    v_normal = buf.raw_view(png(SIZE, SIZE, crate_normal))
    while len(buf.data) % 4:
        buf.data.append(0)

    gltf = {
        "asset": {"version": "2.0", "generator": "april-blink gen_sample_assets.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "Crate", "mesh": 0}],
        "meshes": [
            {
                "name": "Crate",
                "primitives": [
                    {
                        "attributes": {"POSITION": a_pos, "NORMAL": a_nrm, "TEXCOORD_0": a_uv},
                        "indices": a_idx,
                        "material": 0,
                    }
                ],
            }
        ],
        "materials": [
            {
                "name": "CrateWood",
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.8,
                },
                "normalTexture": {"index": 1, "scale": 1.0},
            }
        ],
        "textures": [{"source": 0, "sampler": 0}, {"source": 1, "sampler": 0}],
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
        "images": [
            {"bufferView": v_albedo, "mimeType": "image/png"},
            {"bufferView": v_normal, "mimeType": "image/png"},
        ],
        "bufferViews": buf.views,
        "accessors": buf.accessors,
        "buffers": [{"byteLength": len(buf.data)}],
    }
    json_bytes = json.dumps(gltf, separators=(",", ":")).encode()
    json_bytes += b" " * (-len(json_bytes) % 4)
    bin_bytes = bytes(buf.data)
    glb = (
        struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(json_bytes) + 8 + len(bin_bytes))
        + struct.pack("<II", len(json_bytes), 0x4E4F534A)
        + json_bytes
        + struct.pack("<II", len(bin_bytes), 0x004E4942)
        + bin_bytes
    )
    (OUT / "crate.glb").write_bytes(glb)


# ----------------------------------------------------------------------------------------
# Audio (16-bit mono PCM WAV)
# ----------------------------------------------------------------------------------------

SAMPLE_RATE = 22050


def wav(samples):
    data = b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples)
    fmt = struct.pack("<HHIIHH", 1, 1, SAMPLE_RATE, SAMPLE_RATE * 2, 2, 16)
    return (
        b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
        + b"fmt " + struct.pack("<I", len(fmt)) + fmt
        + b"data" + struct.pack("<I", len(data)) + data
    )


def noise(count, seed):
    """Deterministic white noise in [-1, 1] (LCG)."""
    state = seed
    out = []
    for _ in range(count):
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        out.append(state / 0x3FFFFFFF - 1.0)
    return out


def build_thud():
    count = int(SAMPLE_RATE * 0.35)
    grit = noise(count, 3)
    out, phase, low = [], 0.0, 0.0
    for i in range(count):
        t = i / SAMPLE_RATE
        phase += 2 * math.pi * (55 + 65 * math.exp(-t * 18)) / SAMPLE_RATE
        low += (grit[i] - low) * 0.08  # dull the noise burst
        out.append(0.8 * math.sin(phase) * math.exp(-t * 11) + 1.5 * low * math.exp(-t * 60))
    return out


def build_hum():
    # Every partial completes whole cycles in 2 s, so the loop is seamless.
    count = SAMPLE_RATE * 2
    out = []
    for i in range(count):
        t = i / SAMPLE_RATE
        swell = 0.75 + 0.25 * math.sin(2 * math.pi * 0.5 * t)
        tone = (
            0.5 * math.sin(2 * math.pi * 110 * t)
            + 0.25 * math.sin(2 * math.pi * 165 * t)
            + 0.12 * math.sin(2 * math.pi * 330.5 * t)
        )
        out.append(0.45 * swell * tone)
    return out


def build_wind():
    count = SAMPLE_RATE * 4
    white = noise(count, 11)
    # Two one-pole low-passes run around the loop twice so the filter state at the end
    # matches the start (seamless).
    a = b = 0.0
    out = [0.0] * count
    for _ in range(2):
        for i in range(count):
            a += (white[i] - a) * 0.02
            b += (a - b) * 0.05
            out[i] = b
    peak = max(abs(s) for s in out) or 1.0
    return [
        0.7 * s / peak * (0.6 + 0.4 * math.sin(2 * math.pi * 0.25 * i / SAMPLE_RATE))
        for i, s in enumerate(out)
    ]


def build_audio():
    AUDIO_OUT.mkdir(parents=True, exist_ok=True)
    (AUDIO_OUT / "thud.wav").write_bytes(wav(build_thud()))
    (AUDIO_OUT / "hum.wav").write_bytes(wav(build_hum()))
    (AUDIO_OUT / "wind.wav").write_bytes(wav(build_wind()))


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    build_totem()
    build_crate()
    build_audio()
    for f in sorted([*OUT.iterdir(), *AUDIO_OUT.iterdir()]):
        print(f"{f.relative_to(OUT.parent.parent)}  {f.stat().st_size} bytes")
