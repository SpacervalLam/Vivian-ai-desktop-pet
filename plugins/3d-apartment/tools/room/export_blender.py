"""Publish the saved Blender furniture source as a self-contained runtime GLB."""
import bpy, json, struct
from pathlib import Path
ROOT=Path(bpy.data.filepath).resolve().parents[2]
assert bpy.context.scene.get('vivianFurnitureVersion') == 1
out=ROOT/'room/models/vivian-furniture.glb'
out.parent.mkdir(parents=True,exist_ok=True)
opts=dict(filepath=str(out),export_format='GLB',export_apply=True,
          export_yup=True,export_cameras=False,export_lights=False,
          export_animations=False,export_extras=True,export_vertex_color='ACTIVE',
          export_all_vertex_colors=False,export_image_format='AUTO')
schema=bpy.ops.export_scene.gltf.get_rna_type().properties.keys()
bpy.ops.export_scene.gltf(**{k:v for k,v in opts.items() if k in schema})
data=out.read_bytes()
assert data[:4]==b'glTF' and struct.unpack_from('<I',data,8)[0]==len(data)
doc=json.loads(data[20:20+struct.unpack_from('<I',data,12)[0]])
assert all('COLOR_0' in p['attributes'] for m in doc['meshes'] for p in m['primitives']), 'AO missing'
assert not any('uri' in b for b in doc['buffers']), 'External buffer dependency'
assert not any('uri' in im for im in doc.get('images',[])), 'External image dependency'
assert len(data)<16*1024*1024, 'Runtime asset exceeds 16 MiB budget'
print('EXPORT_OK',len(data),'bytes',flush=True)
