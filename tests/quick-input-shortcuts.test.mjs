import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = await readFile('src-tauri/src/config/manager.rs', 'utf8');
const names = ['default_shortcut', 'default_shortcut_nana', 'default_shortcut_broadcast', 'default_shortcut_memory', 'default_shortcut_chat', 'default_shortcut_settings', 'default_shortcut_room', 'default_shortcut_screen_analyze'];
const functions = names.map(name => {
  const body = source.match(new RegExp(`fn ${name}\\(\\) -> String \\{[\\s\\S]*?\\}`))?.[0];
  assert.ok(body, name);
  return body;
}).join('\n');
const start = source.indexOf('// Upgrade the previous input defaults');
const end = source.indexOf('// ── 配置迁移', start);
assert.ok(start >= 0 && end > start);
const migration = source.slice(start, end);
const policy = await readFile('src-tauri/src/desktop_menu_policy.rs', 'utf8');
const movementExclusion = policy.match(/pub fn enforce_movement_exclusion\([\s\S]*?\n\}/)?.[0];
assert.ok(movementExclusion);
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-input-shortcuts-'));
try {
  const input = path.join(dir, 'shortcuts.rs');
  const executable = path.join(dir, process.platform === 'win32' ? 'shortcuts.exe' : 'shortcuts');
  await writeFile(input, `${functions}
struct Base { shortcut: String, shortcut_nana: String, shortcut_broadcast: String, shortcut_memory: String }
struct Window { smart_positioning_enabled: bool, desktop_physics_enabled: bool }
struct Config { base: Base, window: Window }
mod desktop_menu_policy { ${movementExclusion} }
fn migrate(config: &mut Config) { ${migration} }
fn main() {
  assert_eq!(default_shortcut(), "CommandOrControl+Shift+V");
  assert_eq!(default_shortcut_nana(), "CommandOrControl+Shift+N");
  assert_eq!(default_shortcut_broadcast(), "CommandOrControl+Shift+B");
  let defaults = [${names.map(name => `${name}()`).join(',')}];
  assert_eq!(defaults.iter().collect::<std::collections::HashSet<_>>().len(), defaults.len());
  let mut config = Config { base: Base { shortcut: "Ctrl+Shift+A".into(), shortcut_nana: "Control+Shift+Q".into(), shortcut_broadcast: "CommandOrControl+Shift+Z".into(), shortcut_memory: "CommandOrControl+Shift+N".into() }, window: Window { smart_positioning_enabled: true, desktop_physics_enabled: true } };
  migrate(&mut config);
  assert!(!config.window.smart_positioning_enabled && config.window.desktop_physics_enabled);
  assert_eq!(config.base.shortcut, default_shortcut());
  assert_eq!(config.base.shortcut_nana, default_shortcut_nana());
  assert_eq!(config.base.shortcut_broadcast, default_shortcut_broadcast());
  assert_eq!(config.base.shortcut_memory, default_shortcut_memory());
  config.base.shortcut = "Control+Alt+K".into();
  migrate(&mut config);
  assert_eq!(config.base.shortcut, "Control+Alt+K");
}`);
  const compiled = spawnSync('rustc', ['--edition=2021', input, '-o', executable], { encoding: 'utf8' });
  assert.equal(compiled.status, 0, compiled.stderr);
  const run = spawnSync(executable, [], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  console.log('Actual shortcut defaults and migration compile; inputs are correct, all defaults distinct, custom bindings retained.');
} finally { await rm(dir, { recursive: true, force: true }); }
