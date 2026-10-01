"""Refine the migrated furniture without moving its layout or collider anchors.

Run in background Blender: blender -b --python plugins/3d-apartment/tools/room/build_blender.py
Or with the standalone bpy 4.2 Python module. Only writes inside this project.
The generated .blend is the editable source; export_blender.py publishes it.
"""
import bpy, bmesh, json, math, struct
from pathlib import Path
from mathutils import Vector
from mathutils.bvhtree import BVHTree

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'art/room'
assert bpy.app.background, 'Use a separate background process, never reset a live session.'
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=str(OUT / 'procedural-base.glb'))
scene = bpy.context.scene
furniture = [o for o in scene.objects if o.get('roomAssetId')]
assert len(furniture) == 40, 'Migration manifest changed; audit the selected furniture.'
stats = {'furniture': len(furniture), 'sourceTriangles': 84570}

def descendants(root):
    return [o for o in root.children_recursive if o.type == 'MESH']

# Standalone bpy can leave embedded GLB images unloaded; materialize them before packing.
base=(OUT/'procedural-base.glb').read_bytes()
json_len=struct.unpack_from('<I',base,12)[0]
doc=json.loads(base[20:20+json_len])
binary=base[28+json_len:]
texture_dir=OUT/'textures'
texture_dir.mkdir(exist_ok=True)
for i,entry in enumerate(doc.get('images',[])):
    view=doc['bufferViews'][entry['bufferView']]
    offset=view.get('byteOffset',0)
    path=texture_dir/('surface-%02d.png'%i)
    path.write_bytes(binary[offset:offset+view['byteLength']])
    old=bpy.data.images.get('Image_'+str(i))
    new=bpy.data.images.load(str(path),check_existing=False)
    assert new.size[0]>0, ('Texture decode failed',str(path))
    if old:
        old.user_remap(new)
        bpy.data.images.remove(old)
# Reuse original texture artwork, with local embedded images (no runtime remote assets).
for im in bpy.data.images:
    if im.size[0] > 512 or im.size[1] > 512:
        ratio = 512 / max(im.size)
        im.scale(max(1, int(im.size[0]*ratio)), max(1, int(im.size[1]*ratio)))
    if im.has_data:
        im.pack()

for o in list(scene.objects):
    if o.type != 'MESH':
        continue
    bm = bmesh.new()
    bm.from_mesh(o.data)
    bmesh.ops.remove_doubles(bm, verts=list(bm.verts), dist=0.000001)
    # Remove coplanar subdivisions while preserving normals, seams and texture boundaries.
    bmesh.ops.dissolve_limit(bm, angle_limit=0.001, verts=list(bm.verts), edges=list(bm.edges), delimit={'UV', 'NORMAL', 'MATERIAL'})
    bm.to_mesh(o.data)
    bm.free()
    is_fabric = any(m and m.name.startswith('MAT_Fabric') for m in o.data.materials)
    dims = o.get('dimensions')
    # glTF importer rotates axes through a parent: use actual local bounds for deformation.
    if is_fabric and dims and min(dims) > 0.045 and o.get('primitive') in {'RoundedBoxGeometry','BoxGeometry'}:
        lo = Vector([min(v.co[i] for v in o.data.vertices) for i in range(3)])
        hi = Vector([max(v.co[i] for v in o.data.vertices) for i in range(3)])
        span = hi-lo
        thin = min(range(3), key=lambda i: span[i])
        axes = [i for i in range(3) if i != thin]
        puff = min(0.025, span[thin]*0.18)
        for v in o.data.vertices:
            uv = [(v.co[i]-lo[i])/max(span[i], 0.0001) for i in axes]
            edge = max(0, math.sin(math.pi*uv[0])*math.sin(math.pi*uv[1]))
            side = (v.co[thin]-(lo[thin]+hi[thin])/2)/max(span[thin]/2,0.0001)
            v.co[thin] += puff * edge * side
        for p in o.data.polygons:
            p.use_smooth = True
    o.data.update()

# Add fine seam piping to the existing duvet and seat cushions, inside original footprints.
seam_mat = bpy.data.materials.new('MAT_Stitch_WarmLinen')
seam_mat.diffuse_color = (0.27, 0.31, 0.33, 1)
seam_mat.use_nodes = True
seam_mat.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = seam_mat.diffuse_color
seam_mat.node_tree.nodes.get('Principled BSDF').inputs['Roughness'].default_value = 0.96
seams = 0
for root in furniture:
    if root.get('kind') not in {'jpBed', 'jpLowSofa', 'jpLoungeChair'}:
        continue
    for o in list(descendants(root)):
        if not any(m and m.name.startswith('MAT_Fabric') for m in o.data.materials):
            continue
        dims = o.get('dimensions')
        if not dims or min(dims) < 0.06 or max(dims) < 0.3:
            continue
        lo = Vector([min(v.co[i] for v in o.data.vertices) for i in range(3)])
        hi = Vector([max(v.co[i] for v in o.data.vertices) for i in range(3)])
        span = hi-lo
        thin = min(range(3), key=lambda i: span[i])
        a,b = [i for i in range(3) if i != thin]
        curve = bpy.data.curves.new(o.name+'_Piping', 'CURVE')
        curve.dimensions = '3D'
        curve.bevel_depth = 0.0022
        curve.bevel_resolution = 0
        spline = curve.splines.new('POLY')
        pts=[]
        # Rounded rectangular seam, inset from the silhouette.
        for ca,cb,start in [(1,1,0),(-1,1,90),(-1,-1,180),(1,-1,270)]:
            for step in range(5):
                angle=math.radians(start+step*22.5)
                v=(lo+hi)/2
                ra,rb=span[a]/2-0.02,span[b]/2-0.02
                radius=min(0.03,ra/3,rb/3)
                v[a]+=ca*(ra-radius)+math.cos(angle)*radius
                v[b]+=cb*(rb-radius)+math.sin(angle)*radius
                v[thin]=hi[thin]-span[thin]*0.22
                pts.append(v)
        spline.points.add(len(pts)-1)
        for p,v in zip(spline.points,pts): p.co=(*v,1)
        spline.use_cyclic_u=True
        seam=bpy.data.objects.new(curve.name,curve)
        scene.collection.objects.link(seam)
        seam.parent=o.parent
        seam.matrix_world=o.matrix_world.copy()
        seam.data.materials.append(seam_mat)
        seam['castShadow']=False
        seams+=1

exec(compile((ROOT/'tools/room/author_details.py').read_text(encoding='utf-8'),str(ROOT/'tools/room/author_details.py'),'exec'))

# Join within each furniture/material bucket, retaining each named furniture root.
# This is deliberately never a whole-apartment merge: wall visibility and colliders stay local.
for root in furniture:
    buckets={}
    for o in root.children_recursive:
        if o.type not in {'MESH','CURVE'}: continue
        mat=o.data.materials[0] if o.data.materials else None
        buckets.setdefault(mat,[]).append(o)
    for mat,objects in buckets.items():
        bpy.ops.object.select_all(action='DESELECT')
        for o in objects: o.select_set(True)
        bpy.context.view_layer.objects.active=objects[0]
        bpy.ops.object.convert(target='MESH')
        if len(objects)>1: bpy.ops.object.join()
        obj=bpy.context.view_layer.objects.active
        world=obj.matrix_world.copy()
        obj.parent=root
        obj.matrix_world=world
        obj.name=root['roomAssetId']+'__'+(mat.name if mat else 'Surface')

bpy.context.view_layer.update()
meshes=[o for o in scene.objects if o.type=='MESH']
verts=[]; polys=[]
for o in meshes:
    if any(m and m.surface_render_method=='BLENDED' for m in o.data.materials): continue
    offset=len(verts)
    verts.extend(o.matrix_world@v.co for v in o.data.vertices)
    polys.extend(tuple(offset+i for i in p.vertices) for p in o.data.polygons)
bvh=BVHTree.FromPolygons(verts,polys)
# Short-range ambient occlusion only, not fixed sunlight: remains valid in every time/weather.
ray_count=16
rays=[]
for i in range(ray_count):
    z=(i+0.5)/ray_count
    r=math.sqrt(1-z*z); a=i*2.399963229728653
    rays.append(Vector((r*math.cos(a),r*math.sin(a),z)))
for o in meshes:
    me=o.data
    color=me.color_attributes.new(name='BakedAO',type='FLOAT_COLOR',domain='POINT')
    me.color_attributes.active_color=color
    normal_matrix=o.matrix_world.to_3x3().inverted().transposed()
    for v in me.vertices:
        n=(normal_matrix@v.normal).normalized()
        rotation=Vector((0,0,1)).rotation_difference(n)
        origin=o.matrix_world@v.co+n*0.003
        blocked=0
        for ray in rays:
            hit,_,_,distance=bvh.ray_cast(origin,rotation@ray,0.32)
            if hit is not None: blocked+=1-distance/0.32
        ao=max(0.48,1-0.75*blocked/ray_count)
        color.data[v.index].color=(ao,ao,ao,1)

stats['seams']=seams
stats['meshes']=len(meshes)
stats['triangles']=sum(sum(len(p.vertices)-2 for p in o.data.polygons) for o in meshes)
stats['vertices']=sum(len(o.data.vertices) for o in meshes)
assert stats['triangles'] < 110000, stats
scene['vivianFurnitureVersion']=1
scene['migrationStats']=json.dumps(stats)
bpy.ops.wm.save_as_mainfile(filepath=str(OUT/'vivian-furniture.blend'))
(OUT/'build-report.json').write_text(json.dumps(stats,indent=2),encoding='utf-8')
exec(compile((ROOT/'tools/room/export_blender.py').read_text(encoding='utf-8'),str(ROOT/'tools/room/export_blender.py'),'exec'))
print('ROOM_BUILD_OK', json.dumps(stats), flush=True)
