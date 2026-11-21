"""Expand short motion timelines while retaining every authored key drawing.

Usage: python scripts/chibi/expand-short-motions.py --backup-root <non-repository directory>
Requires Pillow, NumPy and OpenCV. Backups and the build report stay outside the repo.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import time

import cv2
import numpy as np
from PIL import Image

parser = argparse.ArgumentParser()
parser.add_argument('--backup-root', type=Path, required=True)
parser.add_argument('--rewrite-names', help='Re-encode only these comma-separated motions after an interrupted build')
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
backup = args.backup_root.resolve()
if backup == root or root in backup.parents:
    raise ValueError('Backups must be outside the repository')
backup.mkdir(parents=True, exist_ok=True)

def source(relative):
    saved = backup / relative
    if not saved.exists():
        saved.parent.mkdir(parents=True, exist_ok=True)
        saved.write_bytes((root / relative).read_bytes())
    return saved

def read_json(relative):
    return json.loads(source(relative).read_text(encoding='utf-8'))

vocab = read_json('src/chibi/animations.json')
current_vocab = json.loads((root / 'src/chibi/animations.json').read_text(encoding='utf-8'))
for spec in vocab['animations']:
    current = next(s for s in current_vocab['animations'] if s['name'] == spec['name'])
    if current.get('mirrorRight'):
        spec.update(sheet=current['sheet'], mirrorRight=True)
anchors = read_json('src/chibi/frameAnchors.json')
scales = read_json('src/chibi/frameScales.json')
for table in (anchors, scales):
    for key in list(table):
        if key.endswith(('-walk-right-sheet.webp', '-turn-right-sheet.webp')):
            del table[key]
chains = read_json('src/chibi/entryStates.json')
selected = [s for s in vocab['animations'] if 2 <= s['frames'] <= 8]
for spec in vocab['animations']:
    if spec['name'] == 'edge-bottom' and spec['frames'] == 16:
        spec['keyframes'] = [0, 5, 10, 15]
report = []

def load_frames(url, spec):
    sheet = np.array(Image.open(source('public' + url)).convert('RGBA'))
    return [sheet[(i//spec['cols'])*512:(i//spec['cols']+1)*512,
                  (i%spec['cols'])*512:(i%spec['cols']+1)*512].copy()
            for i in range(spec['frames'])]

def fill(spec, character, direction):
    return spec['sheet'].replace('{character}', character).replace('{direction}', direction)

def landmark(frame):
    yy, xx = np.where(frame[:, :, 3] > 200)
    # Follow the central body, excluding swinging props and detached accessories.
    mask = (frame[:, :, 3] > 200)
    mask[:, :160] = False
    mask[:, 352:] = False
    mask[:100] = False
    mask[350:] = False
    body_x = np.where(mask)[1]
    return np.array([np.mean(body_x) if body_x.size else np.mean(xx), yy.max()]), (xx.min(), yy.min(), xx.max(), yy.max())

def translated(frame, offset, box):
    dx = np.clip(offset[0], max(-2, 12-box[0]), min(2, 499-box[2]))
    dy = np.clip(offset[1], max(-2, 12-box[1]), min(2, 499-box[3]))
    if abs(dx) < .01 and abs(dy) < .01:
        return frame.copy()
    rgba = frame.astype(np.float32) / 255
    rgba[:, :, :3] *= rgba[:, :, 3:4]
    result = cv2.warpAffine(rgba, np.float32([[1, 0, dx], [0, 1, dy]]), (512, 512), flags=cv2.INTER_LINEAR)
    result[:, :, :3] /= np.maximum(result[:, :, 3:4], 1/255)
    result = np.clip(np.round(result * 255), 0, 255).astype(np.uint8)
    result[result[:, :, 3] < 8] = 0
    return result

for spec in selected:
    count = spec['frames']
    slots = np.floor(np.arange(count)*16/count).astype(int)
    positions = np.interp(np.arange(16), slots, np.arange(count))
    chosen = np.floor(positions + 1e-6).astype(int)
    times = np.interp(np.arange(17), np.append(slots, 16), np.cumsum([0] + spec['durations']))
    durations = np.diff(np.round(times).astype(int)).tolist()
    if min(durations) <= 0:
        raise ValueError(f"Insufficient playback time: {spec['name']}")
    for character in spec.get('characters', vocab['characters']):
        for direction in (['left'] if spec.get('mirrorRight') else spec.get('directions', ['left'])):
            url = fill(spec, character, direction)
            keys = load_frames(url, spec)
            geometry = [landmark(frame) for frame in keys]
            output = []
            indices = chosen.tolist()
            reverse = next((chain for chain in chains['chains'] if chain['to'] == spec['name'] and character in chain.get('reverseSourceFor', [])), None)
            if reverse:
                upstream = next(s for s in vocab['animations'] if s['name'] == reverse['from'])
                upstream_url = fill(upstream, character, direction)
                upstream_frames = load_frames(upstream_url, upstream)
                indices = np.floor(np.linspace(len(upstream_frames)-1, 0, 16) + .5).astype(int).tolist()
                output = [upstream_frames[i] for i in indices]
                for table in (anchors, scales):
                    if upstream_url in table:
                        table[url] = [table[upstream_url][i] for i in indices]
                    else:
                        table.pop(url, None)
            else:
                for i, position in enumerate(positions):
                    key = chosen[i]
                    if i in slots:
                        output.append(keys[key].copy())
                    else:
                        lo = int(np.floor(position))
                        hi = min(lo+1, count-1)
                        fraction = position-lo
                        destination = geometry[lo][0]*(1-fraction) + geometry[hi][0]*fraction
                        output.append(translated(keys[key], destination-geometry[key][0], geometry[key][1]))
                for table in (anchors, scales):
                    if url in table:
                        old = table[url]
                        table[url] = [old[i] for i in chosen]
                for key, slot in enumerate(slots):
                    assert np.array_equal(output[slot], keys[key]), f'{url}: changed key pose'
            for i, frame in enumerate(output):
                alpha = frame[:, :, 3]
                assert not np.any(alpha[:12]) and not np.any(alpha[-12:])
                assert not np.any(alpha[:, :12]) and not np.any(alpha[:, -12:])
                assert np.sum(alpha > 128) > 3000, f'{url}/{i}: empty drawing'
            sheet = np.zeros((2048, 2048, 4), dtype=np.uint8)
            for i, frame in enumerate(output):
                sheet[(i//4)*512:(i//4+1)*512, (i%4)*512:(i%4+1)*512] = frame
            if not args.rewrite_names or spec['name'] in args.rewrite_names.split(','):
                encoded = backup / 'encoded-sheet.webp'
                Image.fromarray(sheet).save(encoded, lossless=True, method=1)
                for attempt in range(10):
                    try:
                        shutil.copyfile(encoded, root / ('public'+url))
                        break
                    except OSError:
                        if attempt == 9:
                            raise
                        time.sleep(.2)
            unique = len({hashlib.sha256(frame.tobytes()).digest() for frame in output})
            report.append(dict(file=url, originalFrames=count, frames=16, uniqueDrawings=unique, duration=sum(durations)))
            print(f'{character}/{spec["name"]}/{direction}: {count} -> 16 ({unique} unique drawings)', flush=True)
    spec.update(cols=4, rows=4, frames=16, durations=durations, keyframes=slots.tolist())

for relative, data in [('src/chibi/animations.json', vocab), ('src/chibi/frameAnchors.json', anchors), ('src/chibi/frameScales.json', scales)]:
    (root / relative).write_text(json.dumps(data, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
(backup / 'build-report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(f'Expanded {len(report)} sheets; backups and report: {backup}')
