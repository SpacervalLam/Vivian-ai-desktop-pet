"""Author two original vehicles in Blender 4.2; export GLBs and a studio render.
Run with Blender --background --python, or the standalone bpy Python module.
Local +X is forward; Blender Z-up becomes Three.js Y-up through glTF export.
"""
import bpy, math
from pathlib import Path
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'art/vehicles'
PUB = ROOT / 'room/models/vehicles'
OUT.mkdir(parents=True, exist_ok=True)
PUB.mkdir(parents=True, exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)

def material(name, color, metallic=0, rough=.4, emission=0):
    m=bpy.data.materials.new(name);m.diffuse_color=(*color,1);m.use_nodes=True
    p=m.node_tree.nodes.get('Principled BSDF');p.inputs['Base Color'].default_value=(*color,1)
    p.inputs['Metallic'].default_value=metallic;p.inputs['Roughness'].default_value=rough
    if emission:p.inputs['Emission Color'].default_value=(*color,1);p.inputs['Emission Strength'].default_value=emission
    if name=='BodyPaint':p.inputs['Coat Weight'].default_value=.5;p.inputs['Coat Roughness'].default_value=.22
    return m

paint=material('BodyPaint',(.12,.30,.37),.65,.26)
rubber=material('Rubber',(.019,.025,.028),0,.83)
trim=material('Graphite',(.045,.058,.062),.3,.36)
chrome=material('MachinedAluminum',(.53,.59,.63),.9,.24)
glass=material('SmokedGlass',(.045,.11,.15),.42,.12)
white=material('LED',(.84,.95,1),.1,.23,2)
red=material('RearLens',(.55,.015,.025),.25,.22,1.2)
amber=material('Amber',(.95,.34,.025),.2,.3)
plate=material('Plate',(.84,.85,.77),.2,.5)

active_root=None
def finish(o,name,m,bevel=0):
    o.name=name;o.parent=active_root;o.data.materials.append(m)
    if bevel:
        mod=o.modifiers.new('Manufactured radii','BEVEL');mod.width=bevel;mod.segments=3
        bpy.context.view_layer.objects.active=o;bpy.ops.object.modifier_apply(modifier=mod.name)
    if o.type=='MESH':
        for f in o.data.polygons:f.use_smooth=True
        mod=o.modifiers.new('Surface normals','WEIGHTED_NORMAL');mod.keep_sharp=True
        bpy.context.view_layer.objects.active=o;bpy.ops.object.modifier_apply(modifier=mod.name)
    return o

def box(name,loc,size,m,bevel=.025):
    bpy.ops.mesh.primitive_cube_add(size=1,location=loc);o=bpy.context.object;o.dimensions=size
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    return finish(o,name,m,bevel)

def panel(name,vertices,m):
    mesh=bpy.data.meshes.new(name);mesh.from_pydata(vertices,[],[tuple(range(len(vertices)))]);mesh.update()
    o=bpy.data.objects.new(name,mesh);bpy.context.collection.objects.link(o)
    return finish(o,name,m)

def line(name,points,r,m):
    curve=bpy.data.curves.new(name,'CURVE');curve.dimensions='3D';curve.bevel_depth=r;curve.bevel_resolution=2
    s=curve.splines.new('POLY');s.points.add(len(points)-1)
    for p,v in zip(s.points,points):p.co=(*v,1)
    o=bpy.data.objects.new(name,curve);bpy.context.collection.objects.link(o);o.parent=active_root;curve.materials.append(m)
    return o

def loft(name,sections,m):
    # Each ring has sloping shoulders and tucked-under sills, not a scaled box.
    vertices=[]
    for x,w,b,t in sections:
        for sy,z in [(-.82,b),(-1,b+.09),(-1,t-.12),(-.83,t),(.83,t),(1,t-.12),(1,b+.09),(.82,b)]:vertices.append((x,sy*w,z))
    faces=[tuple(range(7,-1,-1))]
    for i in range(len(sections)-1):
        for j in range(8):faces.append((i*8+j,i*8+(j+1)%8,(i+1)*8+(j+1)%8,(i+1)*8+j))
    faces.append(tuple(range((len(sections)-1)*8,len(sections)*8)))
    mesh=bpy.data.meshes.new(name);mesh.from_pydata(vertices,[],faces);mesh.update()
    o=bpy.data.objects.new(name,mesh);bpy.context.collection.objects.link(o);return finish(o,name,m,.045)

def cylinder(name,loc,radius,depth,m):
    bpy.ops.mesh.primitive_cylinder_add(vertices=40,radius=radius,depth=depth,location=loc,rotation=(math.pi/2,0,0))
    return finish(bpy.context.object,name,m,.007)

def wheels(body,axles,w,r):
    for x in axles:
        # Wheel arches are cut right through the body, leaving visible fenders.
        bpy.ops.mesh.primitive_cylinder_add(vertices=48,radius=r+.07,depth=w*2+.4,location=(x,0,r),rotation=(math.pi/2,0,0))
        cut=bpy.context.object;mod=body.modifiers.new('Open wheel arch','BOOLEAN');mod.operation='DIFFERENCE';mod.object=cut
        bpy.context.view_layer.objects.active=body;bpy.ops.object.modifier_apply(modifier=mod.name);bpy.data.objects.remove(cut,do_unlink=True)
        for side in [-1,1]:
            y=side*(w-.09)
            bpy.ops.mesh.primitive_torus_add(major_radius=r-.072,minor_radius=.074,major_segments=48,minor_segments=12,location=(x,y,r),rotation=(math.pi/2,0,0))
            finish(bpy.context.object,'Tire',rubber)
            cylinder('Brake disc',(x,y+side*.047,r),r*.63,.04,trim)
            cylinder('Rim barrel',(x,y+side*.08,r),r*.67,.025,chrome)
            cylinder('Rim recess',(x,y+side*.098,r),r*.56,.01,trim)
            for j in range(10):
                a=j*math.tau/10
                points=[(x+math.sin(a)*r*.18,y+side*.11,r+math.cos(a)*r*.18),(x+math.sin(a+.18)*r*.58,y+side*.115,r+math.cos(a+.18)*r*.58)]
                line('Forged spoke',points,.018,chrome)
            cylinder('Hub',(x,y+side*.12,r),r*.19,.027,chrome)
            for j in range(5):
                a=j*math.tau/5;cylinder('Lug bolt',(x+math.sin(a)*.043,y+side*.14,r+math.cos(a)*.043),.009,.008,trim)
            points=[(x+math.cos(a)* (r+.08),side*(w+.003),r+math.sin(a)*(r+.08)) for a in [j*math.pi/24 for j in range(25)]]
            line('Wheel arch lip',points,.017,paint)

def vehicle(kind):
    global active_root
    active_root=bpy.data.objects.new(kind,None);bpy.context.collection.objects.link(active_root);root=active_root
    van=kind=='van';L=5.2 if van else 4.3;w=.92 if van else .83;r=.36 if van else .32
    body=loft('Sculpted body',[( -L/2,.85*w,.37,.84),(-L/2+.18,w,.29,.98),(-1.4,w,.29,1.02),(.9,w,.29,.91),(L/2-.25,.98*w,.32,.78),(L/2,.85*w,.39,.72)],paint)
    box('Undertray',(0,0,.30),(L-.3,w*1.8,.16),trim,.07)
    if van:
        loft('Cargo shell',[(-2.55,.86,.76,2.42),(-2.38,.90,.7,2.53),(.48,.90,.7,2.53),(.74,.86,.78,2.38)],paint)
        loft('Cabin',[ (.5,.87,.81,2.39),(1.08,.81,.86,2.33),(1.75,.86,.83,1.58),(2.1,.85,.78,1.28)],paint)
        panel('Windshield',[(1.14,-.69,2.30),(1.14,.69,2.30),(1.79,.72,1.575),(1.79,-.72,1.575)],glass)
        window=[(.68,1.42),(.68,2.26),(1.06,2.26),(1.70,1.55),(1.70,1.42)]
        for s in [-1,1]:
            panel('Cab side glass',[(x,s*.886,z) for x,z in window],glass)
            line('Sliding door seam',[(-1.8,s*.906,.83),(-1.8,s*.906,2.32),(.3,s*.906,2.32),(.3,s*.906,.83)],.007,trim)
            line('Cargo belt',[(-2.4,s*.915,1.18),(.4,s*.915,1.18)],.035,trim)
            box('Cargo handle',(.1,s*.924,1.43),(.24,.04,.065),chrome,.02)
        for y in [-.45,.45]:box('Rear door',(-2.558,y,1.61),(.035,.83,1.66),paint,.04)
        axles=[-1.72,1.65]
    else:
        loft('Fastback roof',[(-1.48,.75,.86,1.01),(-.92,.685,.86,1.37),(-.73,.68,.86,1.415),(.16,.68,.84,1.44),(.35,.69,.84,1.40),(1.10,.76,.80,.93)],paint)
        panel('Windshield',[(.39,-.61,1.398),(.39,.61,1.398),(1.03,.68,.997),(1.03,-.68,.997)],glass)
        panel('Rear glass',[(-1.39,-.69,1.04),(-1.39,.69,1.04),(-.87,.626,1.38),(-.87,-.626,1.38)],glass)
        for s in [-1,1]:
            # Glass is inset between visible A/B/C pillars, with a bright belt moulding.
            for coords in [[(-1.32,.99),(-.82,1.345),(-.33,1.357),(-.33,.98)],[(-.27,.98),(-.27,1.357),(.28,1.37),(.93,.975)]]:
                panel('Side glazing',[(x,s*(.774-(z-.97)*.24),z) for x,z in coords],glass)
            line('Window surround',[(-1.36,s*.78,.97),(-.84,s*.68,1.38),(.3,s*.68,1.40),(1.0,s*.78,.955),(-1.36,s*.78,.97)],.012,chrome)
            for x in [-.35,.94]:line('Door shut line',[(x,s*.837,.44),(x,s*.837,.87),(x-.025,s*.784,.95)],.005,trim)
            for x in [-.57,.67]:box('Flush handle',(x,s*.843,.86),(.18,.025,.035),chrome,.011)
            line('Shoulder crease',[(-1.9,s*.79,.87),(-.5,s*.838,.90),(.9,s*.836,.83),(1.9,s*.77,.73)],.008,paint)
        axles=[-1.30,1.32]
    wheels(body,axles,w,r)
    # Recompute broad panel normals after boolean arches to avoid pinched highlights.
    bpy.context.view_layer.objects.active=body
    normal=body.modifiers.new('Final body normals','WEIGHTED_NORMAL');normal.keep_sharp=True;normal.weight=75
    bpy.ops.object.modifier_apply(modifier=normal.name)
    front=L/2
    box('Lower grille',(front-.014,0,.48),(.07,w*1.27,.20),trim,.045)
    for y in [-.5,-.35,-.2,-.05,.1,.25,.4,.5]:box('Grille blade',(front+.026,y,.48),(.019,.022,.13),chrome,.005)
    for s in [-1,1]:
        box('Headlamp housing',(front-.017,s*w*.64,.66),(.055,.34,.105),trim,.024)
        line('LED signature',[(front+.016,s*w*.45,.69),(front+.018,s*w*.81,.675),(front+.012,s*w*.83,.64)],.012,white)
        box('Projector',(front+.016,s*w*.64,.65),(.016,.07,.045),white,.014)
        box('Tail light',(-front-.004,s*w*.66,.84),(.055,.34,.1),red,.025)
        box('Mirror stem',(.78 if not van else 1.30,s*(w+.035),1.02 if not van else 1.51),(.13,.13,.05),trim,.018)
        box('Mirror housing',(.79 if not van else 1.30,s*(w+.095),1.06 if not van else 1.56),(.23,.12,.115),paint,.048)
        box('Mirror glass',(.691 if not van else 1.201,s*(w+.095),1.06 if not van else 1.56),(.012,.093,.078),chrome,.018)
        line('Sill blade',[(-1.0,s*(w+.005),.35),(.98,s*(w+.005),.35)],.025,trim)
    box('License plate',(front+.036,0,.62),(.014,.35,.095),plate,.007)
    box('Rear plate',(-front-.036,0,.61),(.014,.35,.095),plate,.007)
    for y in [-.1,-.045,.01,.065,.12]:box('Plate type',(front+.047,y,.62),(.006,.018,.045),trim,.002)
    line('Wiper',[(1.02 if not van else 1.77,-.57,1.005 if not van else 1.56),(.85 if not van else 1.64,.12,1.12 if not van else 1.71)],.009,trim)
    # Bake bevels and curves; join by material to keep runtime draw calls small.
    bpy.ops.object.select_all(action='DESELECT')
    for o in root.children_recursive:o.select_set(True)
    bpy.context.view_layer.objects.active=next(o for o in root.children_recursive if o.type=='MESH')
    bpy.ops.object.convert(target='MESH')
    for m in [paint,rubber,trim,chrome,glass,white,red,amber,plate]:
        objs=[o for o in root.children_recursive if o.type=='MESH' and o.data.materials and o.data.materials[0]==m]
        if not objs:continue
        bpy.ops.object.select_all(action='DESELECT')
        for o in objs:o.select_set(True)
        bpy.context.view_layer.objects.active=objs[0];bpy.ops.object.join();objs[0].name=kind+'_'+m.name
    bpy.ops.object.select_all(action='DESELECT');root.select_set(True)
    for o in root.children_recursive:o.select_set(True)
    bpy.ops.export_scene.gltf(filepath=str(PUB/(kind+'.glb')),export_format='GLB',use_selection=True,export_yup=True,export_extras=True)
    return root

sedan=vehicle('sedan');van=vehicle('van')
sedan.location=(0,-1.6,0);van.location=(-.8,1.6,0)
# Warm porcelain for the studio van; runtime instances receive their own paint.
van_paint=paint.copy();van_paint.name='VanStudioPaint';van_paint.node_tree.nodes['Principled BSDF'].inputs['Base Color'].default_value=(.75,.70,.59,1)
for o in van.children_recursive:
    if o.type=='MESH' and o.data.materials and o.data.materials[0]==paint:o.data.materials[0]=van_paint
active_root=None
ground=material('Studio floor',(.12,.15,.18),.12,.48)
box('Cyclorama',(0,0,-.12),(200,200,.2),ground,.01)
scene=bpy.context.scene;scene.world=bpy.data.worlds.new('Studio');scene.world.use_nodes=True
scene.world.node_tree.nodes['Background'].inputs[0].default_value=(.24,.28,.34,1)
scene.world.node_tree.nodes['Background'].inputs[1].default_value=.5
for loc,power,size in [((2,-5,7),1600,5),((-4,2,6),2000,4),((4,4,5),1300,3)]:
    bpy.ops.object.light_add(type='AREA',location=loc);o=bpy.context.object;o.data.energy=power;o.data.shape='DISK';o.data.size=size;o.rotation_euler=(Vector((0,0,.7))-o.location).to_track_quat('-Z','Y').to_euler()
bpy.ops.object.camera_add(location=(8,-10,6.4));camera=bpy.context.object;camera.rotation_euler=(Vector((-.2,0,1))-camera.location).to_track_quat('-Z','Y').to_euler();camera.data.type='ORTHO';camera.data.ortho_scale=9.4;scene.camera=camera
scene.render.engine='CYCLES';scene.cycles.samples=32;scene.cycles.use_denoising=True
scene.render.resolution_x=1280;scene.render.resolution_y=960;scene.render.resolution_percentage=100
scene.view_settings.view_transform='AgX';scene.render.image_settings.file_format='PNG';scene.render.filepath=str(OUT/'fleet-studio.png')
bpy.ops.wm.save_as_mainfile(filepath=str(OUT/'lumina-fleet.blend'))
bpy.ops.render.render(write_still=True)
print('Fleet exported and rendered:',PUB,flush=True)
