import bpy
from pathlib import Path
root=Path(__file__).resolve().parents[2]
bpy.ops.wm.open_mainfile(filepath=str(root/'art/room/vivian-furniture.blend'))
for im in bpy.data.images:
    print('IMAGE',im.name,tuple(im.size),im.has_data,len(im.pixels),im.filepath,len(im.packed_file.data) if im.packed_file else None,flush=True)
print('COLOR OPTIONS',[(i.identifier,i.name) for i in bpy.ops.export_scene.gltf.get_rna_type().properties['export_vertex_color'].enum_items])
