import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = path => readFile(`src-tauri/src/${path}`, 'utf8');
const [game, world, agent, driver, perception, system, state] = await Promise.all([
  read('game_companion.rs'), read('world/mod.rs'), read('world/environment_agent.rs'),
  read('world/foreground_window.rs'), read('tools/builtin/perception_tools.rs'),
  read('tools/builtin/extended_system_ops.rs'), read('state.rs'),
]);
// Enforce the ownership boundary as well as the behavioral tests in foreground_context.rs.
assert.doesNotMatch(game, /get_current_foreground_window|get_foreground_window\(|last_external_app\(/);
assert.match(game, /world_provider\.environment\.game_foreground/);
assert.doesNotMatch(world, /cached_foreground_window/);
assert.match(world, /self\.environment\.start_foreground_events\(Arc::downgrade\(self\)\)/);
assert.match(driver, /pub\(super\) fn get_current_foreground_window/);
assert.doesNotMatch(driver, /LAST_EXTERNAL_APP|fn remember_external_app/);
const watch = agent.slice(agent.indexOf('pub fn start_foreground_events'), agent.indexOf('/// This foreground helper'));
assert.match(watch, /foreground_started\.swap/);
assert.match(watch, /drain_foreground_events/);
assert.doesNotMatch(watch, /quiet_reason|proactive\.enabled|priority\.is_idle/,
  'foreground tracking must continue while optional speech/planning is suppressed');
assert.match(perception, /environment\.refresh_foreground\(\)/);
assert.doesNotMatch(perception, /foreground_window::get_current_foreground_window/);
const activeStart = system.indexOf('impl Tool for GetActiveWindowTool');
const activeWindow = system.slice(activeStart, system.indexOf('\n    fn is_read_only(', activeStart));
assert.match(activeWindow, /environment\.refresh_foreground\(\)/);
assert.doesNotMatch(activeWindow, /GetForegroundWindow|silent_command\("powershell"\)/);
assert.match(state, /_environment_registration: environment_registration/,
  'the service registration disposer must live as long as AppState');
const [input, inputPolicy] = await Promise.all([read('world/game_input.rs'), read('world/game_input_policy.rs')]);
assert.match(agent, /game_input: OnceLock<super::game_input::GameInputMonitor>/);
assert.match(agent, /game_input_activity/);
assert.match(agent, /let changed = self\.foreground\.lock\(\)\.observe[\s\S]*?self\.sync_game_input\(\)/,
  'alt-tab must update the environment-owned input scope');
assert.match(input, /RegisterRawInputDevices/);
assert.doesNotMatch(input, /SetWindowsHookEx|SendInput|emit\(|LLMRequest|std::fs|File::/,
  'raw input transport must be passive and must not publish/store raw packets');
assert.match(input, /actual != expected/, 'native input must be scoped to the real focused game');
assert.match(inputPolicy, /const BUCKETS: usize/);
assert.match(game, /game_input_activity\(\)\.allows_comment\(\)/);
assert.equal((game.match(/game_input_activity\(\)\.allows_comment\(\)/g) ?? []).length, 2,
  'operation intensity is checked before generation and again before delivery');
console.log('Foreground ownership: one environment-owned stream/history, shared consumers, planner-independent tracking and retained service registration passed');
