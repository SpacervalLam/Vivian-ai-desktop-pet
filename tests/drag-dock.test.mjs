import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = await readFile('src-tauri/src/commands/window.rs', 'utf8');
const dockSource = await readFile('src-tauri/src/dock.rs', 'utf8');
const canvas = await readFile('src/components/ChibiPetCanvas.tsx', 'utf8');

// 贴边阈值与「拖离边缘」判据都是纯几何/纯状态函数，直接编译它们做真实验证。
const geometry = source.slice(source.indexOf('fn edge_overflow'), source.indexOf('fn ensure_window_visible'));
const pressThreshold = source.match(/const DRAG_START_TOLERANCE_PX[\s\S]*?\n\}/)[0];
assert(geometry.includes('fn drag_dock_edge'));
// 采样起点守卫已迁到 dock 层，window.rs 不该再有一份。
assert.doesNotMatch(geometry, /fn side_overflow/);

const dir = await mkdtemp(path.join(tmpdir(), 'pet-drag-dock-'));
const exe = (name) => path.join(dir, process.platform === 'win32' ? `${name}.exe` : name);

async function compile(name, code) {
  const file = path.join(dir, `${name}.rs`);
  await writeFile(file, code);
  const built = spawnSync('rustc', ['--edition=2021', file, '-o', exe(name)], { encoding: 'utf8' });
  assert.equal(built.status, 0, built.stdout + built.stderr);
  const run = spawnSync(exe(name), [], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stdout + run.stderr);
}

try {
  // 1. 贴边阈值：只认松手那一刻的真实窗口矩形，顶部不吸附。
  await compile('geometry', `${pressThreshold}\n${geometry}
fn main() {
 assert!(!drag_threshold_exceeded((-1200, 700), -1200.0, 700.0), "stationary edge hold");
 assert!(!drag_threshold_exceeded((-1200, 700), -1194.0, 708.0), "minor pointer jitter");
 assert!(drag_threshold_exceeded((-1200, 700), -1194.0, 709.0), "deliberate drag");
 for (left, top, scale) in [(0.0,0.0,1.0),(-2560.0,-400.0,2.0),(1920.0,0.0,1.5)] {
  let w=300.0*scale; let h=300.0*scale;
  let right=left+1920.0*scale; let bottom=top+1080.0*scale;
  let pick=|x,y| drag_dock_edge(x,y,w,h,left,top,right,bottom,scale).map(|(side,_)| side);
  let t=6.0*scale;
  assert_eq!(pick(left-t+1.0,top+100.0),None);
  assert_eq!(pick(left-t,top+100.0),Some("left"));
  assert_eq!(pick(right-w+t-1.0,top+100.0),None);
  assert_eq!(pick(right-w+t,top+100.0),Some("right"));
  assert_eq!(pick(left+100.0,bottom-h+t-1.0),None);
  assert_eq!(pick(left+100.0,bottom-h+t),Some("bottom"));
  assert_eq!(pick(left+100.0,top-h),None);
  assert_eq!(pick(left-2.0*t,bottom-h+t),Some("left"));
  assert_eq!(pick(left+200.0,top+200.0),None);
 }
 assert_eq!(drag_dock_edge(-3.0,100.0,100.0,100.0,0.0,0.0,1920.0,1080.0,1.0),None);
 assert_eq!(drag_dock_edge(-4.0,100.0,100.0,100.0,0.0,0.0,1920.0,1080.0,1.0),Some(("left",4.0)));
}
`);

  // 2. DockController：几何裁决 + epoch，覆盖「时灵时不灵」的三个分叉。
  await compile('dock_state', `${dockSource}
fn main() {
 let a = "pet-a";
 assert_eq!(state(a).mode, DockMode::Off);
 assert_eq!(observe(a, None), DockAction::None);
 assert!(!accept(a, Some(7)));
 assert!(!accept(a, None));

 let e1 = mark_docked(a, "left", 60.0);
 assert_eq!(state(a).mode, DockMode::Docked);
 assert!(state(a).entering, "松手贴边后等入场动画落位结算");
 assert!(accept(a, Some(e1)));
 assert!(accept(a, None));
 // 入场期间窗口会被动画收回到贴边位姿（越界量可能变小），不能判离开。
 assert_eq!(observe(a, Some(("left", 40.0))), DockAction::None);
 assert_eq!(update_docked_overflow(a, 40.0), None, "入场期间不抬阈值");
 // 落位结算：以最终贴边位姿为阈值。
 assert_eq!(settle(a, Some(("left", 60.0))), SettleResult::Docked);
 assert!(!state(a).entering);
 // 沿边缘平移、或往屏外再推一点，都仍算贴着。
 assert_eq!(observe(a, Some(("left", 60.0))), DockAction::None);
 assert_eq!(observe(a, Some(("left", 61.0))), DockAction::None);
 // 往屏内拉 1 像素、完全回屏内、换一条边，都必须立刻恢复。
 assert_eq!(observe(a, Some(("left", 59.0))), DockAction::LeftEdge);
 // 贴边动画把窗口再推出去后，阈值跟到实际最大值；往回拉一点就撤销。
 assert_eq!(update_docked_overflow(a, 90.0), Some(90.0));
 assert_eq!(observe(a, Some(("left", 90.0))), DockAction::None);
 assert_eq!(observe(a, Some(("left", 89.0))), DockAction::LeftEdge);
 assert_eq!(update_docked_overflow(a, 70.0), Some(90.0), "threshold only grows");
 assert_eq!(observe(a, None), DockAction::LeftEdge);
 assert_eq!(observe(a, Some(("right", 90.0))), DockAction::LeftEdge);

 let e2 = leave_edge(a).unwrap();
 assert!(e2 > e1);
 assert_eq!(state(a).mode, DockMode::Off);
 assert_eq!(state(a).undocks, 1);
 assert!(!accept(a, Some(e1)), "late animation frames must never reinstall the region");
 assert!(!accept(a, None), "legacy frames without epoch are rejected once Off");
 assert!(leave_edge(a).is_none());
 assert_eq!(observe(a, None), DockAction::None);

 // 把桌宠「按入」边缘越深越容易回弹：松手越界 200px，贴边位姿只有 106px，
 // 入场动画要把窗口收回来（越界量一路变小），这段变化不能被当成离开边缘。
 let f = "pet-f";
 let _ = mark_docked(f, "left", 200.0);
 for overflow in [190.0, 150.0, 120.0, 106.0] {
  assert_eq!(observe(f, Some(("left", overflow))), DockAction::None);
 }
 assert_eq!(settle(f, Some(("left", 106.0))), SettleResult::Docked);
 assert_eq!(observe(f, Some(("left", 106.0))), DockAction::None);
 assert_eq!(observe(f, Some(("left", 105.0))), DockAction::LeftEdge);

 // 入场结束时窗口已经不在边缘（动画被打断、被挪回屏内）：结算直接判离开。
 let g = "pet-g";
 let _ = mark_docked(g, "left", 200.0);
 assert_eq!(settle(g, None), SettleResult::LeftEdge);
 assert_eq!(settle(g, Some(("left", 200.0))), SettleResult::Ignored, "已经不在入场阶段");

 // Docking 阶段几何本来就没达标：不能被误判成「离开边缘」。
 let b = "pet-b";
 let e3 = begin_docking(b, "right", 60.0);
 assert_eq!(observe(b, None), DockAction::None);
 assert_eq!(observe(b, Some(("right", 59.0))), DockAction::None);
 assert_eq!(observe(b, Some(("left", 90.0))), DockAction::None);
 assert_eq!(observe(b, Some(("right", 60.0))), DockAction::ReachedDock);
 let promoted = promote_docked(b, "right", 60.0);
 assert_eq!(promoted, e3, "reaching the edge must not invalidate in-flight frames");
 assert_eq!(state(b).mode, DockMode::Docked);
 assert!(state(b).entering, "达标后前端还要滑到 dockX，仍属入场");
 assert!(accept(b, Some(e3)));
 assert_eq!(observe(b, Some(("right", 40.0))), DockAction::None);
 assert_eq!(settle(b, Some(("right", 61.0))), SettleResult::Docked);
 assert_eq!(observe(b, Some(("right", 61.0))), DockAction::None);
 let _ = leave_edge(b);
 assert!(!accept(b, Some(e3)), "user grab cancels docking and invalidates its frames");
 let manual = mark_docked(b, "left", 60.0);
 assert!(leave_edge_for_epoch(b, e3).is_none(), "fullscreen exit must preserve a newer manual dock");
 assert_eq!(state(b).epoch, manual);
 assert_eq!(state(b).mode, DockMode::Docked);
 let _ = leave_edge(b);
 let fullscreen = begin_docking(b, "bottom", 60.0);
 assert!(leave_edge_for_epoch(b, fullscreen).is_some(), "fullscreen exit can cancel its own entry");
 assert!(!accept(b, Some(fullscreen)), "cancelled fullscreen frames cannot move the window back");

 // 入场卡死兜底只认同一 epoch 的 Docking。
 let d = "pet-d";
 let e4 = begin_docking(d, "bottom", 60.0);
 assert!(!expired(d, e4, std::time::Duration::from_secs(60)));
 assert!(expired(d, e4, std::time::Duration::from_millis(0)));
 let e5 = mark_docked(d, "bottom", 60.0);
 assert!(!expired(d, e5, std::time::Duration::from_millis(0)));
 let _ = leave_edge(d);
 let _ = begin_docking(d, "left", 60.0);
 assert!(!expired(d, e4, std::time::Duration::from_millis(0)), "stale epoch must not cancel a new entry");
 let _ = leave_edge(d);

 // 角色之间互不影响。
 assert_eq!(state("pet-c").epoch, 0);
 assert_eq!(state("pet-c").mode, DockMode::Off);
}
`);

  // 3. window.rs：裁决挂在 SetWindowPos 之后，不再有 60ms 采样守卫。
  assert.match(source, /observe_window_geometry\(window, x, y\);/);
  // 3a. 手感：松手阈值 4 逻辑像素 / 2%，持续越界 1.2s 自动接管。
  assert.match(source, /const MIN_DOCK_OVERFLOW: f64 = 4\.0;/);
  assert.match(source, /const MIN_DOCK_RATIO: f64 = 0\.02;/);
  assert.match(source, /const DRAG_OVERSHOOT_DOCK_MS: u64 = 1200;/);
  assert.match(source, /fn window_overshoot/);
  assert.match(source, /drag_overshoot_since/);
  assert.match(source, /try_drag_dock\(&app_clone, &win, &label, Some\(auto_overflow\)\)/);
  assert.match(source, /win\.emit_to\(&label, "drag:cancelled", json!\(\{\}\)\)/);
  assert.match(source, /if !undocked && try_drag_dock\(&app_clone, &win, &label, None\)/);
  // 3a2. 长按保持原来的贴边状态，真正拖动后才恢复完整窗口。
  const press = source.slice(source.indexOf('pub fn start_window_drag'), source.indexOf('pub fn stop_window_drag'));
  assert.doesNotMatch(press, /leave_edge|ensure_window_visible|try_drag_dock/);
  assert.match(source, /prev_is_dragging \|\| drag_threshold_exceeded\(press\.cursor_start, c\.x, c\.y\)/);
  assert.match(source, /if is_dragging \{\s*if !prev_is_dragging \{[\s\S]*?"drag:started"[\s\S]*?ensure_window_visible\(&win\);/);
  assert.match(source, /if prev_is_dragging && !is_dragging \{\s*if .* != drag_generation \{[\s\S]*?continue;/);
  assert.match(source, /crate::dock::mark_docked\(/);
  assert.match(source, /crate::dock::leave_edge\(/);
  assert.match(source, /let undocked = crate::dock::state\(&label\)\.undocks > drag_start_undocks;/);
  assert.doesNotMatch(source, /DragDockGuard|drag_dock_guard|leaves_edge_on_release|undocked_this_drag|side_overflow/);
  // 单一事件：旧的三条贴边信号与 presence 切换都从窗口链路撤掉。
  assert.doesNotMatch(source, /pet:dock_requested|pet:undock|"presence:changed"/);
  assert.match(source, /"dock:changed"/);
  for (const origin of ['"origin": "drag"', '"origin": "tool"', '"origin": "entry"', '"mode": "Off"']) {
    assert(source.includes(origin), `missing dock:changed payload ${origin}`);
  }
  const dockCmd = source.slice(source.indexOf('pub fn set_pet_dock_position'), source.indexOf('pub fn clear_pet_dock_clip'));
  assert.match(dockCmd, /epoch: Option<u64>/);
  assert.match(dockCmd, /crate::dock::accept/);
  // 3a3. 入场落位结算：前端滑到贴边位姿后调用，迟到帧按 epoch 丢弃。
  const settleCmd = source.slice(source.indexOf('pub fn settle_pet_dock'), source.indexOf('pub fn clear_window_region'));
  assert.match(settleCmd, /crate::dock::accept/);
  assert.match(settleCmd, /crate::dock::settle/);
  assert.match(settleCmd, /crate::dock::SettleResult::LeftEdge/);
  assert.match(settleCmd, /undock_on_drag\(/);
  const libSource = await readFile('src-tauri/src/lib.rs', 'utf8');
  assert.match(libSource, /commands::window::settle_pet_dock,/);
  const canvas = await readFile('src/components/ChibiPetCanvas.tsx', 'utf8');
  assert.match(canvas, /invoke\('settle_pet_dock', \{ epoch: dockEpochRef\.current \}\)/,
    '入场动画落位后必须通知后端结算，否则贴边阈值停在松手位置');

  // 3b. 主动入场：Docking 阶段、几何达标收尾、拖拽取消与超时兜底。
  const entry = source.slice(source.indexOf('pub fn dock_to_edge'), source.indexOf('/// 启动自定义窗口拖动'));
  assert.match(entry, /crate::dock::begin_docking\(/);
  assert.match(entry, /"mode": "Docking"/);
  assert.match(entry, /spawn_dock_timeout\(/);
  const observeFn = source.slice(source.indexOf('fn observe_window_geometry'), source.indexOf('/// 主动贴边'));
  assert.match(observeFn, /crate::dock::promote_docked\(/, 'entry frames must keep their epoch');
  assert.doesNotMatch(observeFn, /mark_docked\(/);
  const walkCmd = source.slice(source.indexOf('pub fn set_dock_walk_position'), source.indexOf('/// 获取窗口位置'));
  assert.match(walkCmd, /epoch: u64/);
  assert.match(walkCmd, /crate::dock::accept/);
  const lib = await readFile('src-tauri/src/lib.rs', 'utf8');
  assert.match(lib, /commands::window::set_dock_walk_position,/);
  assert.match(lib, /dock_tools::set_app_handle/);

  // 3c. ③ 拆工具：动作与状态分离，三语齐上且语料写意图不写对白。
  const dockTool = await readFile('src-tauri/src/tools/builtin/dock_tools.rs', 'utf8');
  assert.match(dockTool, /"dock_to_edge"/);
  assert.match(dockTool, /crate::commands::window::dock_to_edge\(&app, &context\.char_id\)/);
  assert.match(dockTool, /crate::dock::can_dock\(&context\.char_id\)/);
  assert.match(dockTool, /pub fn can_show_dock_tool/);
  assert.match(dockTool, /should_defer\(&self\) -> bool \{\s*true/);
  for (const branch of ['"zh" =>', '"ja" =>', '_ => self.description()']) {
    assert(dockTool.includes(branch), `missing i18n branch ${branch}`);
  }
  assert.doesNotMatch(dockTool, /我要去忙了|我要睡了/);
  const presenceTool = await readFile('src-tauri/src/tools/builtin/presence_tools.rs', 'utf8');
  assert.match(presenceTool, /const SUPPORTED_PRESENCE_STATES: &\[&str\] = &\["online", "rest"\]/);
  assert.doesNotMatch(presenceTool, /dock_to_edge\(|online\/rest\/edge|when you announce/);
  const brain = await readFile('src-tauri/src/brain/brain.rs', 'utf8');
  assert.match(brain, /`dock_to_edge` is a separate window action/);
  assert.doesNotMatch(brain, /when you announce a state change/);
  const modules = await readFile('src-tauri/src/pipeline/prompt_modules.rs', 'utf8');
  assert.match(modules, /dock_to_edge is a separate window action/);
  assert.doesNotMatch(modules, /Use set_presence_state with online\/rest\/edge/);
  const builtin = await readFile('src-tauri/src/tools/builtin/mod.rs', 'utf8');
  assert.match(builtin, /dock_tools::DockToEdgeTool::new\(\)/);

  // 3d. ④ 动态注入：hidden 挂 resolve_tool_scope，意图命中强制召回。
  const promptStep = await readFile('src-tauri/src/pipeline/steps/prompt.rs', 'utf8');
  assert.match(promptStep, /can_show_dock_tool\(&self\.char_id\)/);
  assert.match(promptStep, /wants_edge_dock/);
  assert.match(promptStep, /insert\("dock_to_edge"\.to_string\(\)\)/);
  const semantic = await readFile('src-tauri/src/emotion/fast_semantic.rs', 'utf8');
  assert.equal((semantic.match(/label: "edge_dock"/g) ?? []).length, 9, 'zh/en/ja 各 3 条贴边意图语料');

  // 4. ⑥ dockState 与恢复路径零写入。
  const store = await readFile('src/stores/useAppStore.ts', 'utf8');
  assert.match(store, /dockState: 'Off' \| 'Docking' \| 'Docked'/);
  assert.match(store, /setDockState: \(state/);
  const app = await readFile('src/App.tsx', 'utf8');
  assert.match(app, /\['drag:started', 'drag:cancelled'\]/);
  assert.match(app, /invoke\('stop_window_drag', \{ cancel: true \}\)/);
  const fullscreen = source.slice(source.indexOf('pub fn is_foreground_fullscreen'), source.indexOf('pub fn debug_log'));
  assert.match(fullscreen, /GetWindowThreadProcessId\(fg, Some\(&mut pid\)\);\s*if pid == std::process::id\(\) \{\s*return Ok\(false\);/);
  const fullscreenDock = source.slice(source.indexOf('pub fn dock_for_fullscreen'), source.indexOf('pub fn restore_fullscreen_dock'));
  assert.match(fullscreenDock, /!is_foreground_fullscreen\(\)\?/);
  assert.match(app, /const dockState = useAppStore\(state => state\.dockState\)/);
  assert((app.match(/dockState === 'Off'/g) ?? []).length >= 3, 'positioning/ambient/physics');
  const hiding = await readFile('src/hooks/useHiding.ts', 'utf8');
  assert.match(hiding, /useAppStore\.getState\(\)\.dockState !== 'Off'/, 'fullscreen docking leaves existing docks alone');
  assert.match(hiding, /invoke\('restore_fullscreen_dock', \{ epoch, x, y \}\)/);
  assert.match(app, /dockState !== 'Off'/);
  const physics = await readFile('src/hooks/useDesktopPhysics.ts', 'utf8');
  assert.match(physics, /watch\('dock:changed'/);

  assert.doesNotMatch(canvas, /pet:dock_requested|pet:undock|retryEdge|lastUndockAtRef|1500/);
  assert.match(canvas, /void add<\{character_id: string; mode: 'Off' \| 'Docking' \| 'Docked'/);
  assert.match(canvas, /store\.setDockState\('Off'\)/);
  assert.match(canvas, /store\.setDockState\(p\.mode\)/);
  assert.match(canvas, /if \(dockState !== 'Off'\)/);
  assert.match(canvas, /\}, \[dockState, dockRevision/);
  assert.match(canvas, /dockEpochRef\.current === 0\) return;/);
  assert.match(canvas, /requestedDockRef\.current \?\? dockSideRef\.current/);
  assert.match(canvas, /invoke\('set_dock_walk_position',\{x,y,epoch:dockEpochRef\.current\}\)/);
  const restore = canvas.slice(canvas.indexOf('const enterVisibleState'), canvas.indexOf("if (dockState !== 'Off')"));
  assert.match(restore, /settleVisibleState\(\)/);
  assert.doesNotMatch(restore, /runSlide|set_pet_dock_position|set_window_position|set_dock_walk_position/,
    'restoring the full window must never move the window');
  const moveDock = canvas.slice(canvas.indexOf('const moveDock'), canvas.indexOf('/** 贴边请求不成立'));
  assert.match(moveDock, /epoch: dockEpochRef\.current/);

  console.log('Dock geometry, epoch admission, single event, dynamic injection, dockState and restore ownership passed.');
} finally {
  await rm(dir, { recursive: true, force: true });
}
