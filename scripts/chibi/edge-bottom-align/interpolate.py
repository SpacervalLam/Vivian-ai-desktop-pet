import sys
import cv2
import numpy as np
from PIL import Image

source, output = sys.argv[1:]
image = np.array(Image.open(source).convert("RGBA"))
keys = [image[:, i * 512:(i + 1) * 512].copy() for i in range(4)]
grid_y, grid_x = np.mgrid[:512, :512].astype(np.float32)
def premultiply(frame):
    result = frame.astype(np.float32) / 255
    result[:, :, :3] *= result[:, :, 3:4]
    return result

def ornament_anchor(frame):
    top = int(np.where(frame[:, :, 3] > 200)[0].min())
    xs = np.where(frame[top:top + 45, :, 3] > 200)[1]
    return top, float(xs.min() + xs.max()) / 2

def translate_drawing(frame, anchor, destination):
    top, center = anchor
    target_top, target_center = destination
    # Move the entire drawing rigidly. Local flow can tear faces, fingers, ears,
    # hair and costume accessories where the next pose occludes them.
    mx = grid_x + center - target_center
    my = grid_y + top - target_top
    return cv2.remap(frame, mx.astype(np.float32), my.astype(np.float32), cv2.INTER_LINEAR)

frames = [keys[0]]
for segment, (a, b) in enumerate(zip(keys, keys[1:])):
    pa, pb = premultiply(a), premultiply(b)
    anchor_a, anchor_b = ornament_anchor(a), ornament_anchor(b)
    for step in range(1, 6):
        if step == 5:
            frames.append(b)
            continue
        t = step / 5
        destination = tuple(x * (1-t) + y * t for x, y in zip(anchor_a, anchor_b))
        wa = translate_drawing(pa, anchor_a, destination)
        wb = translate_drawing(pb, anchor_b, destination)
        # Select one intact drawing rather than dissolving silhouettes into double hands.
        blend = wa if t < .5 else wb
        if segment == 1 and t < .5:
            source_bottom = np.where(a[:, :, 3] > 200)[0].max()
            contact = np.where(b[:, :, 3] > 200)[0].max()
            cutoff = round(source_bottom + (contact - source_bottom) * min(1, 2*t))
            blend[cutoff + 1:] = 0
        blend[:, :, :3] /= np.maximum(blend[:, :, 3:4], 1 / 255)
        frame = np.clip(np.round(blend * 255), 0, 255).astype(np.uint8)
        frame[frame[:, :, 3] < 8] = 0
        # Translation can reach the guard band; never allow it to crop opaque art.
        margin = np.zeros((512,512), dtype=bool)
        margin[:12] = margin[-12:] = True
        margin[:, :12] = margin[:, -12:] = True
        if np.any(frame[:, :, 3][margin] > 200):
            yy, xx = np.where(frame[:, :, 3] > 8)
            dx = max(0, 14-int(xx.min())) - max(0, int(xx.max())-497)
            dy = max(0, 14-int(yy.min())) - max(0, int(yy.max())-497)
            frame = cv2.warpAffine(frame, np.float32([[1,0,dx],[0,1,dy]]), (512,512))
            if np.any(frame[:, :, 3][margin] > 200):
                raise RuntimeError("Interpolated artwork exceeds safety margin")
        frame[margin] = 0
        frames.append(frame)
def hair_tone(frame, character):
    rgb = frame[:, :, :3]
    r, g, b = [rgb[:, :, c].astype(np.float32) for c in range(3)]
    top = int(np.where(frame[:, :, 3] > 200)[0].min())
    mask = (frame[:, :, 3] > 230) & (grid_y < top + 130)
    if character == 'nana':
        mask &= (r > 125) & (g > 115) & (b >= r) & (b-g < 70)
    else:
        mask &= (r > 150) & (g > 110) & (b > 80) & (r > g+5) & (g > b+5) & (r-b < 105)
    if mask.sum() < 500:
        raise RuntimeError('Insufficient head palette samples')
    lab = cv2.cvtColor(rgb, cv2.COLOR_RGB2LAB).astype(np.float32)
    return lab, np.median(lab[mask], axis=0)

character = next((name for name in ('nana', 'vivian') if name in source), None)
if character:
    _, reference_tone = hair_tone(frames[0], character)
    for index in range(1, len(frames)):
        frame = frames[index]
        lab, tone = hair_tone(frame, character)
        light = lab[:, :, 0].copy()
        # Preserve black outlines and white highlights while matching the hair midtone.
        lab[:, :, 0] = np.interp(light, [0, tone[0], 255], [0, reference_tone[0], 255])
        weight = np.clip((light - 32) / 96, 0, 1)
        lab[:, :, 1:] += (reference_tone[1:] - tone[1:]) * weight[:, :, None]
        rgb = cv2.cvtColor(np.clip(np.round(lab), 0, 255).astype(np.uint8), cv2.COLOR_LAB2RGB)
        visible = frame[:, :, 3] > 0
        frame[:, :, :3][visible] = rgb[visible]
        _, actual = hair_tone(frame, character)
        if np.max(np.abs(actual - reference_tone)) > 2:
            raise RuntimeError(f'{character}/{index+1}: palette mismatch {actual} vs {reference_tone}')

sheet = np.zeros((2048, 2048, 4), dtype=np.uint8)
for i, frame in enumerate(frames):
    sheet[(i//4)*512:(i//4+1)*512, (i%4)*512:(i%4+1)*512] = frame
Image.fromarray(sheet).save(output, lossless=True, method=4)
