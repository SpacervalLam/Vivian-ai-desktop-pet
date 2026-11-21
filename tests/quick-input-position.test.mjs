import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = await readFile('src-tauri/src/edge_menu.rs', 'utf8');
const geometry = source.match(/fn quick_input_geometry\([\s\S]*?\n\}/)?.[0];
assert.ok(geometry);
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-quick-input-'));
try {
  const input = path.join(dir, 'position.rs');
  const executable = path.join(dir, process.platform === 'win32' ? 'position.exe' : 'position');
  await writeFile(input, `${geometry}
fn main() {
    let first = quick_input_geometry((800.0, 400.0), (0, 0, 1920, 1040), 1.0);
    let moved = quick_input_geometry((900.0, 500.0), (0, 0, 1920, 1040), 1.0);
    assert_eq!((moved.2 - first.2, moved.3 - first.3), (100, 100));
    for (area, scale) in [((0, 0, 1920, 1040), 1.0), ((-2560, -800, 2560, 1400), 2.0), ((1920, 0, 480, 320), 1.25)] {
        for cursor in [(area.0 as f64, area.1 as f64), ((area.0 as f64 + area.2 as f64 - 1.0), (area.1 as f64 + area.3 as f64 - 1.0))] {
            let (width, height, x, y) = quick_input_geometry(cursor, area, scale);
            assert!(x as f64 >= area.0 as f64 + 8.0 * scale - 0.5);
            assert!(y as f64 >= area.1 as f64 + 8.0 * scale - 0.5);
            assert!(x as f64 + width * scale <= area.0 as f64 + area.2 as f64 - 8.0 * scale + 0.5);
            assert!(y as f64 + height * scale <= area.1 as f64 + area.3 as f64 - 8.0 * scale + 0.5);
        }
    }
}`);
  const compiled = spawnSync('rustc', ['--edition=2021', input, '-o', executable], { encoding: 'utf8' });
  assert.equal(compiled.status, 0, compiled.stderr);
  const run = spawnSync(executable, [], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  console.log('Quick input follows cursor and stays within multi-monitor work areas at different DPI.');
} finally {
  await rm(dir, { recursive: true, force: true });
}
