#!/usr/bin/env python3
"""Generates the engine's sample assets (stdlib only, deterministic output).

    python3 scripts/gen_sample_assets.py            # writes into assets/models/ and assets/audio/

Outputs:
  totem.gltf + totem_albedo.png  Skinned 3-segment totem (3-joint chain) with two animation
                                 clips ("Sway", "Spin"). Buffer embedded as a base64 data URI,
                                 base color texture as an external PNG.
  mannequin.gltf                 Rigged humanoid (Mixamo bone names, faces +Z, 1.8 m) built
                                 from boxes, with clips Idle, Walk, Run, Jump, Attack, Climb
                                 and Fall: the character the script templates and the
                                 locomotion system animate, and that ragdolls map onto.
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
# Mannequin: rigged humanoid with locomotion and combat clips
# ----------------------------------------------------------------------------------------

# (name, parent, local translation); rest rotations are identity, so animation rotations are
# relative to the parent's axes. +X is the character's left, +Z its front.
MANNEQUIN_BONES = [
    ("mixamorig:Hips", None, (0, 1.0, 0)),
    ("mixamorig:Spine", "mixamorig:Hips", (0, 0.1, 0)),
    ("mixamorig:Spine1", "mixamorig:Spine", (0, 0.12, 0)),
    ("mixamorig:Spine2", "mixamorig:Spine1", (0, 0.13, 0)),
    ("mixamorig:Neck", "mixamorig:Spine2", (0, 0.17, 0)),
    ("mixamorig:Head", "mixamorig:Neck", (0, 0.08, 0)),
]
for side, sx in (("Left", 1), ("Right", -1)):
    MANNEQUIN_BONES += [
        (f"mixamorig:{side}Shoulder", "mixamorig:Spine2", (0.06 * sx, 0.12, 0)),
        (f"mixamorig:{side}Arm", f"mixamorig:{side}Shoulder", (0.12 * sx, 0, 0)),
        (f"mixamorig:{side}ForeArm", f"mixamorig:{side}Arm", (0, -0.28, 0)),
        (f"mixamorig:{side}Hand", f"mixamorig:{side}ForeArm", (0, -0.25, 0)),
    ]
for side, sx in (("Left", 1), ("Right", -1)):
    MANNEQUIN_BONES += [
        (f"mixamorig:{side}UpLeg", "mixamorig:Hips", (0.09 * sx, -0.05, 0)),
        (f"mixamorig:{side}Leg", f"mixamorig:{side}UpLeg", (0, -0.43, 0)),
        (f"mixamorig:{side}Foot", f"mixamorig:{side}Leg", (0, -0.42, 0)),
        (f"mixamorig:{side}ToeBase", f"mixamorig:{side}Foot", (0, -0.07, 0.12)),
    ]


def mannequin_world():
    """World position of every bone at rest."""
    index = {name: i for i, (name, _, _) in enumerate(MANNEQUIN_BONES)}
    world = []
    for name, parent, t in MANNEQUIN_BONES:
        base = world[index[parent]] if parent else (0, 0, 0)
        world.append(tuple(base[k] + t[k] for k in range(3)))
    return index, world


# (bone, centre, half size, skin?) — boxes rigidly bound to one bone each.
def mannequin_parts():
    parts = [
        ("mixamorig:Hips", (0, 0.98, 0), (0.15, 0.08, 0.09), False),
        ("mixamorig:Spine", (0, 1.15, 0), (0.135, 0.08, 0.085), False),
        ("mixamorig:Spine2", (0, 1.34, 0), (0.17, 0.12, 0.1), False),
        ("mixamorig:Neck", (0, 1.55, 0), (0.04, 0.05, 0.04), True),
        ("mixamorig:Head", (0, 1.68, 0.01), (0.09, 0.11, 0.1), True),
        # A visor shows which way the head faces.
        ("mixamorig:Head", (0, 1.7, 0.105), (0.065, 0.025, 0.015), False),
    ]
    for side, sx in (("Left", 1), ("Right", -1)):
        x = 0.18 * sx
        parts += [
            (f"mixamorig:{side}Arm", (x, 1.33, 0), (0.045, 0.13, 0.045), False),
            (f"mixamorig:{side}ForeArm", (x, 1.07, 0), (0.04, 0.12, 0.04), True),
            (f"mixamorig:{side}Hand", (x, 0.88, 0.0), (0.035, 0.065, 0.025), True),
            (f"mixamorig:{side}UpLeg", (0.09 * sx, 0.73, 0), (0.065, 0.2, 0.065), False),
            (f"mixamorig:{side}Leg", (0.09 * sx, 0.31, 0), (0.05, 0.2, 0.05), False),
            (f"mixamorig:{side}Foot", (0.09 * sx, 0.04, 0.05), (0.05, 0.04, 0.11), False),
        ]
    return parts


def quat(axis, angle):
    s = math.sin(angle / 2)
    return [axis[0] * s, axis[1] * s, axis[2] * s, math.cos(angle / 2)]


def quat_mul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]


X, Y, Z = (1, 0, 0), (0, 1, 0), (0, 0, 1)


def euler(x=0.0, y=0.0, z=0.0):
    """Rotation about X, then Y, then Z (parent axes)."""
    return quat_mul(quat(Z, z), quat_mul(quat(Y, y), quat(X, x)))


# Clips: (name, seconds, looping, pose(t in 0..1) -> {bone: (x, y, z) radians}). Signs:
# bones pointing down swing forwards with -X, bones pointing up lean forwards with +X;
# +Z on the left arm raises it sideways (-Z on the right).
def pose_idle(t):
    breath = math.sin(t * math.tau)
    return {
        "mixamorig:Spine2": (0.03 * breath, 0, 0),
        "mixamorig:LeftArm": (0, 0, 0.08 + 0.02 * breath),
        "mixamorig:RightArm": (0, 0, -0.08 - 0.02 * breath),
        "mixamorig:Head": (-0.02 * breath, 0, 0),
    }


def gait(t, stride, knee, arm, elbow, lean, bounce_twist):
    phase = t * math.tau
    s = math.sin(phase)
    pose = {
        "mixamorig:LeftUpLeg": (-stride * s, 0, 0),
        "mixamorig:RightUpLeg": (stride * s, 0, 0),
        # The knee bends while the leg swings through (behind the body).
        "mixamorig:LeftLeg": (knee * max(0.0, math.sin(phase + 1.2)), 0, 0),
        "mixamorig:RightLeg": (knee * max(0.0, math.sin(phase + 1.2 + math.pi)), 0, 0),
        "mixamorig:LeftFoot": (0.25 * max(0.0, math.sin(phase - 0.6)), 0, 0),
        "mixamorig:RightFoot": (0.25 * max(0.0, math.sin(phase - 0.6 + math.pi)), 0, 0),
        "mixamorig:LeftArm": (arm * s, 0, 0.1),
        "mixamorig:RightArm": (-arm * s, 0, -0.1),
        "mixamorig:LeftForeArm": (-elbow, 0, 0),
        "mixamorig:RightForeArm": (-elbow, 0, 0),
        "mixamorig:Spine": (lean, bounce_twist * s, 0),
        "mixamorig:Spine2": (0, -bounce_twist * s, 0),
    }
    return pose


def pose_walk(t):
    return gait(t, 0.45, 0.7, 0.35, 0.25, 0.04, 0.08)


def pose_run(t):
    return gait(t, 0.85, 1.5, 0.8, 1.3, 0.2, 0.15)


def pose_jump(t):
    # Crouch, push off with arms thrown up, tuck, land.
    crouch = math.sin(min(t / 0.3, 1.0) * math.pi) if t < 0.3 else 0.0
    air = math.sin((t - 0.3) / 0.7 * math.pi) if t >= 0.3 else 0.0
    return {
        "mixamorig:LeftUpLeg": (-0.9 * crouch - 0.5 * air, 0, 0),
        "mixamorig:RightUpLeg": (-0.9 * crouch - 0.3 * air, 0, 0),
        "mixamorig:LeftLeg": (1.4 * crouch + 0.9 * air, 0, 0),
        "mixamorig:RightLeg": (1.4 * crouch + 0.7 * air, 0, 0),
        "mixamorig:LeftFoot": (-0.4 * crouch, 0, 0),
        "mixamorig:RightFoot": (-0.4 * crouch, 0, 0),
        "mixamorig:Spine": (0.35 * crouch, 0, 0),
        "mixamorig:LeftArm": (0.5 * crouch - 2.4 * air, 0, 0.2),
        "mixamorig:RightArm": (0.5 * crouch - 2.4 * air, 0, -0.2),
    }


def pose_attack(t):
    # Wind up, punch with the right arm, recover.
    wind = math.sin(min(t / 0.35, 1.0) * math.pi / 2) if t < 0.35 else 1.0
    hit = math.sin(min(max((t - 0.35) / 0.2, 0.0), 1.0) * math.pi / 2) if t >= 0.35 else 0.0
    back = max(0.0, (t - 0.65) / 0.35)
    reach = hit * (1.0 - back)
    windup = wind * (1.0 - hit)
    return {
        "mixamorig:Spine": (0.1 * reach, 0.5 * windup - 0.5 * reach, 0),
        "mixamorig:RightArm": (0.4 * windup - 1.55 * reach, 0, -0.15),
        "mixamorig:RightForeArm": (-1.9 * windup - 0.1 * reach, 0, 0),
        "mixamorig:LeftArm": (-0.9 * (windup + reach), 0, 0.2),
        "mixamorig:LeftForeArm": (-1.8 * (windup + reach) * 0.8, 0, 0),
        "mixamorig:LeftUpLeg": (-0.3 * reach, 0, 0),
        "mixamorig:RightUpLeg": (0.2 * reach, 0, 0),
        "mixamorig:LeftLeg": (0.3 * reach, 0, 0),
    }


def pose_climb(t):
    s = math.sin(t * math.tau)
    return {
        "mixamorig:LeftArm": (-2.7 + 0.35 * s, 0, 0.15),
        "mixamorig:RightArm": (-2.7 - 0.35 * s, 0, -0.15),
        "mixamorig:LeftForeArm": (-0.5 - 0.4 * max(0.0, -s), 0, 0),
        "mixamorig:RightForeArm": (-0.5 - 0.4 * max(0.0, s), 0, 0),
        "mixamorig:LeftUpLeg": (-0.9 - 0.4 * s, 0, 0),
        "mixamorig:RightUpLeg": (-0.9 + 0.4 * s, 0, 0),
        "mixamorig:LeftLeg": (1.3 + 0.3 * s, 0, 0),
        "mixamorig:RightLeg": (1.3 - 0.3 * s, 0, 0),
        "mixamorig:Head": (-0.4, 0, 0),
    }


def pose_fall(t):
    s = math.sin(t * math.tau * 2)
    return {
        "mixamorig:LeftArm": (-0.6 + 0.3 * s, 0, 1.2),
        "mixamorig:RightArm": (-0.6 - 0.3 * s, 0, -1.2),
        "mixamorig:LeftUpLeg": (-0.4 + 0.2 * s, 0, 0.1),
        "mixamorig:RightUpLeg": (-0.2 - 0.2 * s, 0, -0.1),
        "mixamorig:LeftLeg": (0.6, 0, 0),
        "mixamorig:RightLeg": (0.9, 0, 0),
    }


MANNEQUIN_CLIPS = [
    ("Idle", 2.4, pose_idle),
    ("Walk", 1.0, pose_walk),
    ("Run", 0.66, pose_run),
    ("Jump", 0.8, pose_jump),
    ("Attack", 0.6, pose_attack),
    ("Climb", 1.1, pose_climb),
    ("Fall", 0.8, pose_fall),
]


def build_mannequin():
    buf = BufferBuilder()
    index, world = mannequin_world()
    # One primitive per material: suit and skin.
    prims = {False: [[], [], [], [], [], []], True: [[], [], [], [], [], []]}
    for bone, centre, half, skin in mannequin_parts():
        positions, normals, uvs, indices, joints, weights = prims[skin]
        p, n, t, idx = box(centre, half)
        base = len(positions)
        positions += p
        normals += n
        uvs += t
        indices += [i + base for i in idx]
        joints += [[index[bone], 0, 0, 0]] * len(p)
        weights += [[1.0, 0.0, 0.0, 0.0]] * len(p)
    primitives = []
    for skin, material in ((False, 0), (True, 1)):
        positions, normals, uvs, indices, joints, weights = prims[skin]
        primitives.append(
            {
                "attributes": {
                    "POSITION": buf.accessor(positions, "VEC3", FLOAT, ARRAY_BUFFER, bounds=True),
                    "NORMAL": buf.accessor(normals, "VEC3", FLOAT, ARRAY_BUFFER),
                    "TEXCOORD_0": buf.accessor(uvs, "VEC2", FLOAT, ARRAY_BUFFER),
                    "JOINTS_0": buf.accessor(joints, "VEC4", USHORT, ARRAY_BUFFER),
                    "WEIGHTS_0": buf.accessor(weights, "VEC4", FLOAT, ARRAY_BUFFER),
                },
                "indices": buf.accessor(indices, "SCALAR", UINT, ELEMENT_ARRAY_BUFFER),
                "material": material,
            }
        )
    ibm = [[1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -w[0], -w[1], -w[2], 1] for w in world]
    a_ibm = buf.accessor(ibm, "MAT4", FLOAT)

    # Nodes: 0 the model, 1.. the bones, last the mesh.
    first_bone = 1
    nodes = [{"name": "Mannequin", "children": [first_bone, first_bone + len(MANNEQUIN_BONES)]}]
    for i, (name, parent, t) in enumerate(MANNEQUIN_BONES):
        node = {"name": name}
        if any(c != 0 for c in t):
            node["translation"] = list(t)
        children = [first_bone + c for c, (_, p, _) in enumerate(MANNEQUIN_BONES) if p == name]
        if children:
            node["children"] = children
        nodes.append(node)
    nodes.append({"name": "Body", "mesh": 0, "skin": 0})

    animations = []
    samples = 16
    for name, seconds, pose in MANNEQUIN_CLIPS:
        times = [seconds * k / samples for k in range(samples + 1)]
        a_time = buf.accessor(times, "SCALAR", FLOAT)
        buf.accessors[a_time]["min"] = [0.0]
        buf.accessors[a_time]["max"] = [seconds]
        poses = [pose(k / samples) for k in range(samples + 1)]
        bones = sorted({b for p in poses for b in p})
        samplers, channels = [], []
        for bone in bones:
            rotations = [euler(*p.get(bone, (0, 0, 0))) for p in poses]
            a_rot = buf.accessor(rotations, "VEC4", FLOAT)
            samplers.append({"input": a_time, "output": a_rot, "interpolation": "LINEAR"})
            channels.append(
                {"sampler": len(samplers) - 1, "target": {"node": first_bone + index[bone], "path": "rotation"}}
            )
        animations.append({"name": name, "samplers": samplers, "channels": channels})

    gltf = {
        "asset": {"version": "2.0", "generator": "april-blink gen_sample_assets.py"},
        "scene": 0,
        "scenes": [{"name": "Mannequin", "nodes": [0]}],
        "nodes": nodes,
        "skins": [
            {
                "name": "Skeleton",
                "joints": [first_bone + i for i in range(len(MANNEQUIN_BONES))],
                "inverseBindMatrices": a_ibm,
                "skeleton": first_bone,
            }
        ],
        "meshes": [{"name": "Body", "primitives": primitives}],
        "materials": [
            {
                "name": "Suit",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.18, 0.24, 0.36, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.6,
                },
            },
            {
                "name": "Skin",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.8, 0.6, 0.48, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.7,
                },
            },
        ],
        "animations": animations,
        "bufferViews": buf.views,
        "accessors": buf.accessors,
        "buffers": [
            {
                "byteLength": len(buf.data),
                "uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(buf.data)).decode(),
            }
        ],
    }
    (OUT / "mannequin.gltf").write_text(json.dumps(gltf, indent=1) + "\n")


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
    build_mannequin()
    build_crate()
    build_audio()
    for f in sorted([*OUT.iterdir(), *AUDIO_OUT.iterdir()]):
        print(f"{f.relative_to(OUT.parent.parent)}  {f.stat().st_size} bytes")
