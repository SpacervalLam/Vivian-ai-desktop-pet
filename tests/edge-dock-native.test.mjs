import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

if (process.platform === 'win32') {
  const source = await readFile('src-tauri/src/commands/window.rs','utf8');
  const dock = await readFile('src-tauri/src/dock.rs', 'utf8');
  const command = source.match(/pub fn set_pet_dock_position\([\s\S]*?\n\}/)?.[0];
  const clearClip = source.match(/pub fn clear_pet_dock_clip\([\s\S]*?\n\}/)?.[0];
  const clearRegion = source.match(/fn clear_window_region\([\s\S]*?\n\}/)?.[0];
  const fullscreen = source.match(/pub fn is_foreground_fullscreen\([\s\S]*?\n\}/)?.[0];
  const move = source.match(/fn move_window\([\s\S]*?\n\}/)?.[0];
  assert(command);
  assert(clearClip);
  assert(clearRegion);
  assert(fullscreen);
  assert.match(move, /SWP_NOACTIVATE \| SWP_NOZORDER \| SWP_NOSIZE/, 'docking moves the window at its original size');
  const dir = await mkdtemp(path.join(tmpdir(),'pet-dock-native-'));
  try {
    await writeFile(path.join(dir, 'dock.rs'), dock);
    await writeFile(path.join(dir,'Cargo.toml'), `[package]
name="pet-dock-native-check"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
[dependencies]
tracing="0.1"
windows={version="0.61",features=["Win32_Foundation","Win32_Graphics_Gdi","Win32_UI_Shell","Win32_UI_WindowsAndMessaging"]}
`);
    await writeFile(path.join(dir,'lib.rs'), `#![allow(dead_code)]
mod dock;
mod tauri {
 pub struct WebviewWindow;
 pub struct Size {pub width:u32,pub height:u32}
 impl WebviewWindow {
  pub fn label(&self)->&str{"pet"}
  pub fn outer_size(&self)->Result<Size,String>{Ok(Size{width:300,height:300})}
  pub fn hwnd(&self)->Result<windows::Win32::Foundation::HWND,String>{Ok(Default::default())}
 }
}
fn move_window(_: &tauri::WebviewWindow,_:i32,_:i32){}
use tauri::WebviewWindow;
fn err_str(error: impl std::fmt::Display)->String{error.to_string()}
${clearRegion}
${command}
${clearClip}
${fullscreen}
`);
    const result = spawnSync('cargo',['check','--offline','--quiet','--manifest-path',path.join(dir,'Cargo.toml'),
      '--target-dir',path.resolve('tmp/edge-dock-native-target')],{encoding:'utf8',env:{...process.env,CARGO_PROFILE_DEV_DEBUG:'0',CARGO_BUILD_JOBS:'1'}});
    assert.equal(result.status,0,result.stdout+result.stderr);
    console.log('Production docking and fullscreen detection compile against Win32 APIs.');

    // 无边框窗口在贴边区域被裁过之后，任何一次样式重写都不能把标题栏带回来。
    const taoState = await readFile('src-tauri/vendor/tao/src/platform_impl/windows/window_state.rs','utf8');
    const applyDiff = taoState.match(/fn apply_diff\([\s\S]*?\n  \}/)?.[0] ?? '';
    assert.match(applyDiff, /to_adjusted_window_styles\(\)/,
      'apply_diff must strip frame styles for undecorated windows');
    assert.ok(applyDiff.indexOf('SWP_FRAMECHANGED') < applyDiff.lastIndexOf('ShowWindow'),
      'undecorated windows must recalculate the frame before being shown');

    // WM_NCACTIVATE 不能落到 DefWindowProc，否则透明窗口会被画上标题栏。
    const taoEvents = await readFile('src-tauri/vendor/tao/src/platform_impl/windows/event_loop.rs','utf8');
    const ncActivate = taoEvents.match(/win32wm::WM_NCACTIVATE => \{[\s\S]*?\n    \}/)?.[0] ?? '';
    assert.match(ncActivate, /MARKER_DECORATIONS/);
    assert.match(ncActivate, /ProcResult::Value\(LRESULT\(1\)\)/);

    // 应用侧的 NC 绘制保护只吞 WM_NCPAINT，WM_NCACTIVATE 必须留给 tao 维护激活状态。
    const guard = await readFile('src-tauri/src/borderless.rs','utf8');
    assert.match(guard, /WM_NCPAINT => LRESULT\(0\)/);
    assert.doesNotMatch(guard, /WM_NCACTIVATE =>/);

    const lib = await readFile('src-tauri/src/lib.rs','utf8');
    assert.match(lib, /\.shadow\(false\)[\s\S]{0,120}\.visible\(false\);/,
      'character windows must be created hidden so the NC guard lands before the first show');
    assert.match(lib, /crate::borderless::guard\(&win\);\s*let _ = win\.set_ignore_cursor_events\(false\);\s*let _ = win\.show\(\);/,
      'character windows must install the NC guard before their first show');

    const app = await readFile('src/App.tsx','utf8');
    assert.match(app, /guardBorderless\(fullLabel\)\.then\(\(\) => win\.show\(\)/,
      'frontend child windows must install the NC guard before showing');
    console.log('Borderless windows keep their frame styles stripped across dock/show/hide.');
  } finally { await rm(dir,{recursive:true,force:true}); }
}
