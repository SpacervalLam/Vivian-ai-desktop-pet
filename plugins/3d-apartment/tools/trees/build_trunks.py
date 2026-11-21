"""Author two stylised tree-trunk models in Blender 4.2; export GLBs and studio renders.

Run with the standalone bpy Python module (there is no blender.exe in this repo):

    $env:PYTHONPATH='G:\\vivian-rs\\tmp\\blender-python'
    & "C:\\Users\\86139\\AppData\\Local\\Programs\\Python\\Python311\\python.exe" scripts\\trees\\build_trunks.py

Why these shapes and not a tapered cylinder
-------------------------------------------
The runtime trunks used to be one scaled cylinder (or, in cityDressing, one scaled
cube) per tree.  A cylinder reads as a "pipe" because it has no root flare, no bark
relief and no bend.  Three cues do most of the work and all three are baked in here:

  1. Buttress roots - discrete radial fins that swell the base to ~1.9x the trunk
     radius and fade out by ~22% of the height.  This is the strongest "this is a
     tree" cue at eye level, and it has to be *discrete*: a uniform trumpet flare
     reads as a vase, which is exactly what the first pass looked like.
  2. Bark ridges - the cross-section is a ridged star, not a circle.  The ridge
     profile is |sin| so the grooves have a sharp bottom and a rounded crown, and
     the phase drifts with height so the ridges wander instead of running dead
     straight.
  3. A gentle lean plus a slight S - the axis is a curve, so the silhouette is not
     a trapezoid.

Deliberately *not* baked: the bark colour.  The scene is toon-shaded and each module
already owns its own bark colour (riverbank #62594b, cityDressing #7a5c45, ...), so
the mesh ships with UVs only and the runtime multiplies a shared greyscale bark
texture by the local colour.  See foliage.trunkBarkTexture() and blenderTrunks.ts.

Local +Z is up and the origin sits on the ground plane, so after export_yup=True the
origin is at the tree foot and +Y is up - exactly what the instancing matrix wants.
Blender Z-up becomes Three.js Y-up through the glTF export.
"""
import bpy, math
from pathlib import Path
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'art/trees'
PUB = ROOT / 'room/models/trees'
OUT.mkdir(parents=True, exist_ok=True)
PUB.mkdir(parents=True, exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)


# ---------------------------------------------------------------- materials ---
def material(name, color, rough=.78):
    m = bpy.data.materials.new(name)
    m.diffuse_color = (*color, 1)
    m.use_nodes = True
    p = m.node_tree.nodes.get('Principled BSDF')
    p.inputs['Base Color'].default_value = (*color, 1)
    p.inputs['Roughness'].default_value = rough
    p.inputs['Metallic'].default_value = 0
    return m


bark = material('Bark', (.235, .185, .140))


# ------------------------------------------------------------------ profile ---
def hash01(*vals):
    """Deterministic [0,1) hash - no global random state, so both variants stay stable."""
    h = 2166136261
    for v in vals:
        h = (h ^ (int(v * 100003) & 0xFFFFFFFF)) * 16777619 & 0xFFFFFFFF
    return (h >> 8) / 16777216.0


# Rings are packed near the base where the fins and the flare live - the flare is a
# long, gradual blend, so it needs sample density or it reads as a gown hem.
T_RINGS = [-0.075, -0.030, 0.0, 0.012, 0.028, 0.050, 0.080, 0.118, 0.165,
           0.222, 0.290, 0.368, 0.452, 0.542, 0.636, 0.730, 0.822, 0.908, 1.0]


def trunk_mesh(name, H, R, *, taper, flare, root, root_t, root_n, root_phase,
               ridges, ridge_amp, bend, bend_dir, sway, seed, seg=44):
    """Build one trunk as a ring mesh.

    H / R are the real height and base radius of the model, so callers scale by
    (r_target/R, h_target/H, r_target/R) and the proportions stay honest.
    """
    verts, uvs, faces = [], [], []
    top_z = {}

    for j, t in enumerate(T_RINGS):
        tc = max(t, 0.0)                       # below-ground rings reuse the t=0 profile
        # 1. taper - held back with t**1.5 and a small coefficient, so the trunk keeps
        #    its thickness and does not read as a horn
        prof = 1.0 - taper * tc ** 1.5
        # 2. ground flare - deliberately weak; the fins below carry the shape
        fl = 1.0 + flare * max(0.0, 1.0 - tc / 0.20) ** 2.0
        # 3. ridge phase drifts with height so the grooves wander
        ph = 1.15 * math.sin(tc * 6.283 * 1.9 + seed) + 0.55 * math.sin(tc * 6.283 * 4.3 + seed * 2)
        for i in range(seg):
            th = i * math.tau / seg
            # 4. buttress roots - discrete fins, not a uniform trumpet
            lobe = max(0.0, math.cos(root_n * (th - root_phase))) ** 3
            rt = 1.0 + root * max(0.0, 1.0 - tc / root_t) ** 1.7 * lobe
            # 5. bark ridges - |sin| gives a sharp groove bottom; the 0.85 exponent
            #    widens the crowns so the ridges read as rounded bark, not knife cuts
            ridge = abs(math.sin(ridges * th + ph)) ** 0.85
            amp = ridge_amp * (0.80 + 0.40 * math.sin(ridges * th * 1.5 + seed))
            fl_ = 1.0 + amp * (2.0 * ridge - 1.0)
            # 6. break the machine regularity (small - large values read as melted wax)
            jit = 1.0 + (hash01(i, j, seed) - 0.5) * 0.032
            r = R * prof * fl * rt * fl_ * jit
            # 7. the axis is a curve: a lean plus a slight S
            cx = bend * tc ** 1.7 * math.cos(bend_dir) + sway * math.sin(tc * 2.9 + seed)
            cy = bend * tc ** 1.7 * math.sin(bend_dir) + sway * math.cos(tc * 2.4 + seed)
            z = t * H
            if t >= 1.0:
                z += (hash01(i, 99, seed) - 0.5) * H * 0.018
                top_z[i] = z
            verts.append((cx + r * math.cos(th), cy + r * math.sin(th), z))
            uvs.append((i / seg, (t + 0.06) / 1.12))

    n = seg
    # sides - winding (j,i) -> (j,i+1) -> (j+1,i+1) -> (j+1,i) gives outward normals
    for j in range(len(T_RINGS) - 1):
        for i in range(n):
            a, b = j * n + i, j * n + (i + 1) % n
            c, d = (j + 1) * n + (i + 1) % n, (j + 1) * n + i
            faces.append((a, b, c, d))
    # bottom cap (normal -Z)
    faces.append(tuple(range(n - 1, -1, -1)))
    # top cap: fan to a slightly raised centre so the stump reads as broken, not sawn
    last = (len(T_RINGS) - 1) * n
    centre = len(verts)
    verts.append((0.0, 0.0, sum(top_z.values()) / n + H * 0.016))
    uvs.append((0.5, 0.5))
    for i in range(n):
        faces.append((last + i, last + (i + 1) % n, centre))

    me = bpy.data.meshes.new(name)
    me.from_pydata(verts, [], faces)
    me.update()
    ob = bpy.data.objects.new(name, me)
    bpy.context.collection.objects.link(ob)

    # UVs straight from the (theta, t) we generated with - no unwrap op needed.
    uv = me.uv_layers.new(name='UVMap')
    for loop in me.loops:
        uv.data[loop.index].uv = uvs[loop.vertex_index]

    ob.data.materials.append(bark)
    # ONE material for the whole trunk, including the top cap. The runtime throws the
    # GLB material away and substitutes toon(local bark colour, bark map), so a second
    # material would only split the mesh into two glTF primitives - and the loader
    # would then pick up just the first one, leaving a hole in the top. A pale "cut
    # wood" cap would also read as a felled stump, which is wrong for a living tree
    # whose branches leave below the top.
    for poly in me.polygons:
        poly.use_smooth = True
    return ob


# Two variants, matching the two trunk specs that foliage.plantTree() emits.
#   sakura - the squat, heavily flared park/street cherry  (h 2.42, r 0.150)
#   green  - the slimmer riverside / street tree           (h 2.15, r 0.130)
# ridges must be an integer so the cross-section closes seamlessly at theta = 2pi.
# The root fin spread is capped by the scene's tree pit: 0.7 x 0.7 m, so the base
# must stay near ~0.65 m across or the fins overhang the kerb.
SPECS = {
    'trunk-sakura': dict(H=2.42, R=0.150, taper=.27, flare=.13, root=1.00, root_t=.30,
                         root_n=5, root_phase=.35, ridges=6, ridge_amp=.130,
                         bend=.055, bend_dir=1.25, sway=.030, seed=3.1),
    'trunk-green': dict(H=2.15, R=0.130, taper=.31, flare=.10, root=.85, root_t=.27,
                        root_n=6, root_phase=1.9, ridges=7, ridge_amp=.115,
                        bend=.042, bend_dir=-0.7, sway=.026, seed=8.4),
}

built = {}
for kind, spec in SPECS.items():
    ob = trunk_mesh(kind, **spec)
    bpy.context.view_layer.objects.active = ob
    ob.select_set(True)
    # weighted normals keep the ridge crowns crisp without faceting the round body
    mod = ob.modifiers.new('Bark normals', 'WEIGHTED_NORMAL')
    mod.keep_sharp = True
    mod.weight = 45
    bpy.ops.object.modifier_apply(modifier=mod.name)
    ob.select_set(False)
    # The GLB ships at REAL scale (a 2.42 m trunk is a 2.42 m trunk - you can open it
    # and measure it). The runtime instances it with scale(r, len, r), which needs the
    # unit convention, so it reads these two numbers back off the node's extras and
    # divides them out. Without them the trunk would come out 2.42x too big.
    ob['trunkH'] = float(spec['H'])
    ob['trunkR'] = float(spec['R'])
    built[kind] = ob
    dim = ob.dimensions
    print(f'{kind}: {len(ob.data.polygons)} faces, bbox={[round(v, 3) for v in dim]}', flush=True)

# ------------------------------------------------------------------- export ---
for kind, ob in built.items():
    bpy.ops.object.select_all(action='DESELECT')
    ob.select_set(True)
    bpy.context.view_layer.objects.active = ob
    bpy.ops.export_scene.gltf(filepath=str(PUB / (kind + '.glb')), export_format='GLB',
                              use_selection=True, export_yup=True, export_extras=True,
                              export_apply=True)

# ------------------------------------------------------------------- studio ---
# Park the two trunks side by side on a cyclorama so the shapes can be eyeballed.
built['trunk-sakura'].location = (-0.55, 0.0, 0.0)
built['trunk-green'].location = (0.55, 0.0, 0.0)
ground = material('Studio floor', (.16, .17, .18), .62)
bpy.ops.mesh.primitive_plane_add(size=200, location=(0, 0, -0.02))
floor = bpy.context.object
floor.name = 'Cyclorama'
floor.data.materials.append(ground)

scene = bpy.context.scene
scene.world = bpy.data.worlds.new('Studio')
scene.world.use_nodes = True
scene.world.node_tree.nodes['Background'].inputs[0].default_value = (.30, .33, .38, 1)
scene.world.node_tree.nodes['Background'].inputs[1].default_value = .30
# Sunlight-ish key from above/left plus a soft fill: a tight area light at this range
# blows the bark out and hides exactly the relief we are trying to inspect.
for loc, power, size in [((3.4, -5.0, 6.2), 420, 6), ((-4.4, 2.4, 3.0), 190, 5),
                         ((1.6, 4.4, 2.2), 120, 3)]:
    bpy.ops.object.light_add(type='AREA', location=loc)
    o = bpy.context.object
    o.data.energy = power
    o.data.shape = 'DISK'
    o.data.size = size
    o.rotation_euler = (Vector((0, 0, 1.05)) - o.location).to_track_quat('-Z', 'Y').to_euler()

scene.render.engine = 'CYCLES'
scene.cycles.samples = 64
scene.cycles.use_denoising = True
scene.render.resolution_x = 1280
scene.render.resolution_y = 960
scene.render.resolution_percentage = 100
scene.view_settings.view_transform = 'AgX'
try:
    scene.view_settings.look = 'AgX - Base Contrast'
except TypeError:
    pass   # look names differ between Blender builds; the default is fine
scene.render.image_settings.file_format = 'PNG'


def shoot(name, loc, look, ortho, samples=64):
    scene.cycles.samples = samples
    bpy.ops.object.camera_add(location=loc)
    cam = bpy.context.object
    cam.rotation_euler = (Vector(look) - cam.location).to_track_quat('-Z', 'Y').to_euler()
    cam.data.type = 'ORTHO'
    cam.data.ortho_scale = ortho
    scene.camera = cam
    scene.render.filepath = str(OUT / name)
    bpy.ops.render.render(write_still=True)
    bpy.data.objects.remove(cam, do_unlink=True)
    print('rendered', name, flush=True)


shoot('trunks-studio.png', (4.6, -5.8, 3.2), (0, 0, 1.10), 4.2)
shoot('trunks-base-closeup.png', (1.4, -1.9, 0.50), (0, 0, 0.26), 1.35)
shoot('trunks-upper-closeup.png', (1.3, -1.8, 2.05), (0, 0, 1.90), 0.95)

bpy.ops.wm.save_as_mainfile(filepath=str(OUT / 'tree-trunks.blend'))
print('Trunks exported:', PUB, flush=True)
