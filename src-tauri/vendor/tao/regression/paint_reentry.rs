// Regression probe for the tao Windows paint-reentrancy assertion.
//
// Builds two tao windows, then forces the exact nesting that crashed the app:
// a nested WM_PAINT shows up while an outer paint flush is already dispatching
// messages, and a synchronous SendMessage delivers WM_PAINT to tao's thread
// message target in the middle of it. Focus handling plus the IME text services
// produce the same shape (SetForegroundWindow -> SendMessageW ->
// CtfImeDispatchDefImeMessage -> DispatchMessageW) in the field.
//
// Exit codes: 0 survived, 2 assertion panic detected, 3 probe never nested.

#![allow(non_snake_case, non_camel_case_types)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::time::Instant;

use tao::{
  dpi::LogicalSize,
  event::{Event, StartCause},
  event_loop::{ControlFlow, EventLoop},
  platform::{run_return::EventLoopExtRunReturn, windows::WindowExtWindows},
  window::WindowBuilder,
};

type HWND = *mut c_void;
type LRESULT = isize;

const WM_PAINT: u32 = 0x000F;
const RDW_INVALIDATE: u32 = 0x0001;

// Enough rounds to hit the nested case many times, cheap enough to stay fast.
const MIN_INJECTIONS: usize = 100;
const MAX_INJECTIONS: usize = 300;
const DEADLINE_SECS: u64 = 10;

type SubclassProc = unsafe extern "system" fn(HWND, u32, usize, LRESULT, usize, usize) -> LRESULT;

#[link(name = "user32")]
extern "system" {
  fn FindWindowW(class: *const u16, name: *const u16) -> HWND;
  fn SendMessageW(hwnd: HWND, msg: u32, w: usize, l: LRESULT) -> LRESULT;
  fn RedrawWindow(hwnd: HWND, rect: *const c_void, rgn: *const c_void, flags: u32) -> i32;
  fn GetWindowThreadProcessId(hwnd: HWND, pid: *mut u32) -> u32;
}

#[link(name = "comctl32")]
extern "system" {
  fn SetWindowSubclass(hwnd: HWND, proc: SubclassProc, id: usize, data: usize) -> i32;
  fn DefSubclassProc(hwnd: HWND, msg: u32, w: usize, l: LRESULT) -> LRESULT;
}

static THREAD_TARGET: AtomicIsize = AtomicIsize::new(0);
static HWND_B: AtomicIsize = AtomicIsize::new(0);
static INJECTIONS: AtomicUsize = AtomicUsize::new(0);
static PANICKED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn probe(
  hwnd: HWND,
  msg: u32,
  w: usize,
  l: LRESULT,
  id: usize,
  _data: usize,
) -> LRESULT {
  if msg == WM_PAINT && id == 1 {
    // Dirty the second window before tao handles this paint, so the flush that
    // follows always picks up its WM_PAINT and nests the dispatch.
    RedrawWindow(HWND_B.load(Ordering::SeqCst) as HWND, std::ptr::null(), std::ptr::null(), RDW_INVALIDATE);
  }

  // Win32 subclasses run most-recently-installed first, so this returns after
  // tao's own handling: the runner is parked in its redraw phase at that point.
  let result = DefSubclassProc(hwnd, msg, w, l);

  if msg == WM_PAINT {
    let n = INJECTIONS.fetch_add(1, Ordering::SeqCst);
    if n < MAX_INJECTIONS {
      SendMessageW(THREAD_TARGET.load(Ordering::SeqCst) as HWND, WM_PAINT, 0, 0);
    }
  }

  result
}

fn wide(s: &str) -> Vec<u16> {
  s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
  std::panic::set_hook(Box::new(|info| {
    PANICKED.store(true, Ordering::SeqCst);
    eprintln!("[PANIC] {info}");
  }));

  let mut event_loop = EventLoop::new();
  let a = WindowBuilder::new()
    .with_title("paint-reentry-A")
    .with_inner_size(LogicalSize::new(96u32, 96u32))
    .build(&event_loop)
    .expect("window A");
  let b = WindowBuilder::new()
    .with_title("paint-reentry-B")
    .with_inner_size(LogicalSize::new(96u32, 96u32))
    .build(&event_loop)
    .expect("window B");

  let ha = a.hwnd() as HWND;
  let hb = b.hwnd() as HWND;
  HWND_B.store(hb as isize, Ordering::SeqCst);

  let class = wide("Tao Thread Event Target");
  let target = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
  assert!(!target.is_null(), "tao thread event target not found");
  let mut pid = 0u32;
  unsafe { GetWindowThreadProcessId(target, &mut pid) };
  assert_eq!(pid, std::process::id(), "thread event target belongs to another process");
  THREAD_TARGET.store(target as isize, Ordering::SeqCst);

  assert_ne!(unsafe { SetWindowSubclass(ha, probe, 1, 0) }, 0, "subclass A");
  assert_ne!(unsafe { SetWindowSubclass(hb, probe, 2, 0) }, 0, "subclass B");

  let start = Instant::now();
  let mut armed = false;

  event_loop.run_return(|event, _, control_flow| {
    *control_flow = ControlFlow::Wait;

    // Continuous redraws keep StartCause on WindowEvent, so poll the deadline
    // and the injection budget from every event instead of relying on timers.
    let n = INJECTIONS.load(Ordering::SeqCst);
    if n >= MAX_INJECTIONS || start.elapsed().as_secs() >= DEADLINE_SECS {
      *control_flow = ControlFlow::Exit;
      return;
    }

    if !armed {
      if let Event::NewEvents(StartCause::Init) = event {
        armed = true;
        unsafe {
          RedrawWindow(ha, std::ptr::null(), std::ptr::null(), RDW_INVALIDATE);
        }
      }
      return;
    }

    if let Event::RedrawRequested(_) = event {
      unsafe {
        RedrawWindow(ha, std::ptr::null(), std::ptr::null(), RDW_INVALIDATE);
        RedrawWindow(hb, std::ptr::null(), std::ptr::null(), RDW_INVALIDATE);
      }
    }
  });

  let n = INJECTIONS.load(Ordering::SeqCst);
  drop((a, b));

  if PANICKED.load(Ordering::SeqCst) {
    println!("RESULT: panic injections={n}");
    std::process::exit(2);
  }
  if n < MIN_INJECTIONS {
    println!("RESULT: inconclusive injections={n}");
    std::process::exit(3);
  }
  println!("RESULT: survived injections={n}");
}
