#!/usr/bin/env python3
"""把 3D 房间角色 GLB 的内嵌贴图压到 1024² 原地重打包。

背景:10dae27 把角色 GLB 换成 Tripo AI 导出版,内嵌 4096² JPEG。
在部分 WebView2/显卡环境下,4096² 贴图 createImageBitmap 解码失败,
GLTFLoader 容忍式加载让角色成了白模。压到 1024² 后解码在任何环境都稳,
顺带省下约 40MB 显存(RoomScene 的 MODEL_MAP_MAX=1024 本来就是目标值)。

用法:python plugins/3d-apartment/tools/room/repack_character_textures.py
"""
import io
import json
import struct
from pathlib import Path

from PIL import Image

GLB_MAGIC = 0x46546C67  # 'glTF'
JSON_TYPE = 0x4E4F534A
BIN_TYPE = 0x004E4942
MAX_SIZE = 1024
JPEG_QUALITY = 90

TARGETS = [
    Path(__file__).resolve().parent.parent.parent / "room/vivian_qver.glb",
    Path(__file__).resolve().parent.parent.parent / "room/nana_qver.glb",
]


def parse_glb(data: bytes):
    magic, version, length = struct.unpack("<III", data[:12])
    assert magic == GLB_MAGIC, "not a GLB"
    off = 12
    clen, ctype = struct.unpack("<II", data[off : off + 8])
    off += 8
    assert ctype == JSON_TYPE
    gltf = json.loads(data[off : off + clen].decode("utf-8"))
    off += clen
    blen, btype = struct.unpack("<II", data[off : off + 8])
    off += 8
    assert btype == BIN_TYPE
    return gltf, bytearray(data[off : off + blen])


def pack_glb(gltf, binblob: bytes) -> bytes:
    json_bytes = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
    while len(json_bytes) % 4:
        json_bytes += b" "
    binbytes = bytes(binblob)
    while len(binbytes) % 4:
        binbytes += b"\x00"
    total = 12 + 8 + len(json_bytes) + 8 + len(binbytes)
    out = struct.pack("<III", GLB_MAGIC, 2, total)
    out += struct.pack("<II", len(json_bytes), JSON_TYPE) + json_bytes
    out += struct.pack("<II", len(binbytes), BIN_TYPE) + binbytes
    return out


def repack(path: Path) -> None:
    data = path.read_bytes()
    gltf, binblob = parse_glb(data)
    assert gltf.get("buffers") and len(gltf["buffers"]) == 1

    imgs = gltf["images"]
    assert len(imgs) == 1, "expected exactly one embedded image per GLB"
    img = imgs[0]
    bv = gltf["bufferViews"][img["bufferView"]]
    start = bv.get("byteOffset", 0)
    end = start + bv["byteLength"]
    raw = bytes(binblob[start:end])
    image = Image.open(io.BytesIO(raw))
    old_size = image.size
    if max(old_size) <= MAX_SIZE:
        print(f"{path.name}: 已是 {old_size},跳过")
        return

    image = image.convert("RGB").resize((MAX_SIZE, MAX_SIZE), Image.LANCZOS)
    buf = io.BytesIO()
    image.save(buf, "JPEG", quality=JPEG_QUALITY)
    new_img = buf.getvalue()

    old_len = bv["byteLength"]
    delta = len(new_img) - old_len
    img_bv_idx = img["bufferView"]

    # 在 BIN 里把旧贴图字节原位替换成新贴图
    binblob[start:end] = new_img
    # 贴图区之后(bufferView 起始 >= 旧图结束)的所有视图整体平移 delta
    for i, v in enumerate(gltf["bufferViews"]):
        if i == img_bv_idx:
            v["byteLength"] = len(new_img)
        elif v.get("byteOffset", 0) >= end:
            v["byteOffset"] += delta
    gltf["buffers"][0]["byteLength"] += delta

    out_path = path
    out_path.write_bytes(pack_glb(gltf, binblob))

    # 回读校验:GLB 结构可解析、贴图像素尺寸正确
    img2 = Image.open(io.BytesIO(_read_image(out_path)))
    assert img2.size == (MAX_SIZE, MAX_SIZE), img2.size
    print(
        f"{path.name}: {old_size} -> {img2.size} | "
        f"{len(data) // 1024}KB -> {out_path.stat().st_size // 1024}KB"
    )


def _read_image(path: Path):
    gltf, binblob = parse_glb(path.read_bytes())
    bv = gltf["bufferViews"][gltf["images"][0]["bufferView"]]
    start = bv.get("byteOffset", 0)
    return bytes(binblob[start : start + bv["byteLength"]])


if __name__ == "__main__":
    for target in TARGETS:
        assert target.exists(), f"missing {target}"
        repack(target)
    print("done")