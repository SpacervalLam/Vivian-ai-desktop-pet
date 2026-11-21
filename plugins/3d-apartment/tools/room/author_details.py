"""Blender-authored desk props and fabric finishes; executed before AO and batching."""
def material(name, color, roughness=0.7, metalness=0, emission=0):
    m=bpy.data.materials.new('MAT_'+name);m.use_nodes=True
    bs=m.node_tree.nodes.get('Principled BSDF')
    bs.inputs['Base Color'].default_value=(*color,1)
    bs.inputs['Roughness'].default_value=roughness
    bs.inputs['Metallic'].default_value=metalness
    if emission:
        bs.inputs['Emission Color'].default_value=(*color,1)
        bs.inputs['Emission Strength'].default_value=emission
    return m

cream=material('ABS_Cream',(0.64,0.58,0.46),0.48)
charcoal=material('Graphite',(0.026,0.033,0.035),0.48)
teal=material('Keycaps_Sage',(0.22,0.36,0.32),0.62)
screen=material('CRT_Phosphor',(0.06,0.18,0.15),0.3,emission=0.8)
pixels=material('CRT_Text',(0.36,0.67,0.47),0.65,emission=0.8)
brass=material('Brushed_Brass',(0.35,0.25,0.12),0.34,0.65)
paper=material('Paper_Edge',(0.72,0.69,0.60),0.95)
terra=material('Ceramic_Terracotta',(0.45,0.19,0.11),0.32)
glow=material('Lamp_Warm',(1,0.63,0.27),0.75,emission=1.6)

def box(root,name,pos,size,mat,radius=0.008):
    bpy.ops.mesh.primitive_cube_add(size=1)
    o=bpy.context.object;o.name=root['roomAssetId']+'_'+name
    o.parent=root;o.location=(pos[0],-pos[2],pos[1])
    o.scale=(size[0],size[2],size[1])
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    o.data.materials.append(mat)
    if radius:
        bevel=o.modifiers.new('Soft edges','BEVEL')
        bevel.width=min(radius,min(size)*0.4);bevel.segments=3
        bpy.ops.object.modifier_apply(modifier=bevel.name)
        for p in o.data.polygons:p.use_smooth=True
        normals=o.modifiers.new('Weighted normals','WEIGHTED_NORMAL')
        normals.keep_sharp=True
        bpy.ops.object.modifier_apply(modifier=normals.name)
    return o

def cylinder(root,name,pos,r1,r2,height,mat):
    bpy.ops.mesh.primitive_cone_add(vertices=20,radius1=r1,radius2=r2,depth=height)
    o=bpy.context.object;o.name=root['roomAssetId']+'_'+name
    o.parent=root;o.location=(pos[0],-pos[2],pos[1]);o.data.materials.append(mat)
    for p in o.data.polygons:p.use_smooth=len(p.vertices)==4
    return o

for root in furniture:
    if root.get('kind')!='jpDesk':continue
    W,H,D=root['size']
    inv=root.matrix_world.inverted()
    # Replace only desktop clutter; keep the existing frame, drawers and footprint.
    for o in list(descendants(root)):
        p=inv@o.matrix_world.translation
        if p.z>H-0.018:bpy.data.objects.remove(o,do_unlink=True)
    x=-W*0.06
    box(root,'Monitor_Foot',(x,H+0.016,-0.07),(0.20,0.032,0.16),cream)
    box(root,'Monitor_Neck',(x,H+0.06,-0.10),(0.075,0.08,0.07),cream)
    box(root,'CRT_Shell',(x,H+0.235,-0.09),(0.34,0.29,0.24),cream,0.025)
    box(root,'CRT_Bezel',(x,H+0.25,0.033),(0.284,0.214,0.012),charcoal,0.018)
    box(root,'CRT_Glass',(x,H+0.25,0.041),(0.25,0.179,0.012),screen,0.022)
    for i in range(6):
        box(root,'Terminal_Line',(x-0.026,H+0.307-i*0.016,0.048),(0.13-(i%3)*0.029,0.003,0.0015),pixels,0)
    box(root,'Floppy_Slot',(x+0.048,H+0.116,0.032),(0.105,0.009,0.008),charcoal,0.002)
    box(root,'Keyboard',(x,H+0.021,D*0.32),(0.34,0.029,0.12),cream)
    for row in range(4):
        for col in range(12):
            box(root,'Keycap',(x-0.145+col*0.026,H+0.040,D*0.32-0.04+row*0.026),(0.019,0.011,0.018),teal,0)
    box(root,'Spacebar',(x,H+0.041,D*0.32+0.047),(0.135,0.012,0.014),teal,0.003)
    lx=-W*0.38;lz=-D*0.26
    cylinder(root,'Lamp_Base',(lx,H+0.012,lz),0.065,0.065,0.024,brass)
    cylinder(root,'Lamp_Stem',(lx,H+0.15,lz),0.007,0.007,0.28,brass)
    cylinder(root,'Lamp_Shade',(lx,H+0.31,lz),0.082,0.042,0.09,teal)
    cylinder(root,'Lamp_Diffuser',(lx,H+0.266,lz),0.076,0.076,0.003,glow)
    for i in range(3):
        book=box(root,'Notebook',(W*0.30,H+0.015+i*0.024,-D*0.23),(0.17,0.020,0.12),[terra,paper,teal][i],0.003)
        book.rotation_euler.z=(i-1)*0.07
    cx=W*0.32;cz=D*0.23
    cylinder(root,'Cup',(cx,H+0.045,cz),0.030,0.035,0.09,terra)
    cylinder(root,'Coffee',(cx,H+0.091,cz),0.029,0.029,0.002,charcoal)
    bpy.ops.mesh.primitive_torus_add(major_segments=16,minor_segments=6,major_radius=0.024,minor_radius=0.005)
    handle=bpy.context.object;handle.name=root['roomAssetId']+'_CupHandle'
    handle.parent=root;handle.location=(cx+0.038,-cz,H+0.05);handle.rotation_euler.x=math.pi/2
    handle.data.materials.append(terra)

# Raise the reflectance of navy upholstery, retaining its original woven texture.
for m in bpy.data.materials:
    if m.name.startswith('MAT_Fabric'):
        for node in m.node_tree.nodes:
            if node.type=='MIX' and node.data_type=='RGBA':
                for socket in node.inputs:
                    if socket.type=='RGBA' and not socket.is_linked:
                        c=socket.default_value
                        if max(c[:3])<0.5:
                            socket.default_value=(max(c[0],0.14),max(c[1],0.20),max(c[2],0.25),c[3])

# Replace rectangular pillow/duvet cores with genuinely rounded Blender upholstery.
for root in furniture:
    if root.get('kind')=='jpBed':
        for o in list(descendants(root)):
            dims=o.get('dimensions')
            if not dims or o.get('primitive') not in {'BoxGeometry','RoundedBoxGeometry'}: continue
            pillow=0.065<dims[1]<0.105 and 0.25<dims[0]<0.4
            duvet=0.09<dims[1]<0.14 and dims[0]>0.8
            if not (pillow or duvet): continue
            lo=Vector([min(v.co[i] for v in o.data.vertices) for i in range(3)])
            hi=Vector([max(v.co[i] for v in o.data.vertices) for i in range(3)])
            span=hi-lo
            mats=list(o.data.materials)
            bpy.ops.mesh.primitive_cube_add(size=1)
            temp=bpy.context.object
            temp.scale=span
            bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
            bevel=temp.modifiers.new('Upholstery roundover','BEVEL')
            bevel.width=min(span)*0.44;bevel.segments=4
            bpy.ops.object.modifier_apply(modifier=bevel.name)
            subdiv=temp.modifiers.new('Soft cloth surface','SUBSURF');subdiv.levels=1
            bpy.ops.object.modifier_apply(modifier=subdiv.name)
            for v in temp.data.vertices:
                v.co+=(lo+hi)/2
            for poly in temp.data.polygons: poly.use_smooth=True
            o.data=temp.data.copy()
            for mat in mats:o.data.materials.append(mat)
            bpy.data.objects.remove(temp,do_unlink=True)
    if root.get('kind')=='jpFloorLamp':
        for o in descendants(root):
            dims=o.get('dimensions')
            if not dims or not 0.40<dims[1]<0.44:continue
            m=o.data.materials[0].copy();m.name='MAT_Paper_Luminous'
            bs=m.node_tree.nodes.get('Principled BSDF')
            bs.inputs['Emission Color'].default_value=(0.8,0.52,0.24,1)
            bs.inputs['Emission Strength'].default_value=0.8
            bs.inputs['Roughness'].default_value=0.92
            o.data.materials.clear();o.data.materials.append(m)
