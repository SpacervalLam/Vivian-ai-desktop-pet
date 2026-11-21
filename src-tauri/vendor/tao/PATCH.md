# Windows paint re-entrancy assertion

Vendored from crates.io tao 0.35.3, with its original MIT/Apache-2.0 licenses.

The application's crash log ends in
`event_loop.rs:2344: assertion failed: flush_paint_messages(...)`
from `thread_event_target_callback`. Version 0.35.3 removed the guard on that
path: the comment next to it claims the handler cannot be re-entered because
`flush_paint_messages` never paints the thread event target, but the loop it
runs calls `PeekMessageW`, and `PeekMessageW` delivers pending sent messages
before it returns. Against the live application the sequence was
`PeekMessageW` -> `SetForegroundWindow` -> `SendMessageW` ->
`CtfImeDispatchDefImeMessage` -> `DispatchMessageW`, which lands a `WM_PAINT`
on the thread event target while the runner is already in its redraw phase.
The assertion then aborts the event loop and takes the process down.

Keep both paint paths behaving the same way: only the invocation that owns the
flush finishes the redraw cycle, a nested one returns without touching the
state machine. The public window handler at the same file already models this
by branching on the `flush_paint_messages` return value; the thread target
handler now does the same through `flush_thread_paint_messages`.

The patch is selected by `[patch.crates-io]` in the application manifest; no
machine-local Cargo registry files are modified. Remove the override after
upgrading to an upstream release that handles the nested case, then rerun the
regression below before deleting `vendor/tao`.

## Borderless windows must never paint a title bar

The desktop pets, bubbles, toasts and edge panels are `decorations(false)`
transparent windows. Windows still sends them `WM_NCACTIVATE` / `WM_NCPAINT`,
and `to_window_styles` always sets `WS_CAPTION`, relying on `WM_NCCALCSIZE`
returning 0 to hide the frame. `DefWindowProc(WM_NCACTIVATE)` then paints a real
title bar on top of the client area. On a per-pixel transparent window that
paint is never covered by the WebView, so the pet keeps an extra title bar,
loses part of its client area and shows clipped content after docking. The
title bar reappears whenever such a window is shown or activated again, which
is exactly what happens when a docked pet replies.

Four changes in `window_state.rs` / `event_loop.rs` keep the frame gone:

- `WindowFlags::apply_diff` rewrites the window style with
  `to_adjusted_window_styles`, so a style pass can never re-add `WS_CAPTION` /
  `WS_THICKFRAME` to an undecorated window.
- The `VISIBLE` branch moved after the frame recalculation (`SWP_FRAMECHANGED`),
  matching upstream tao's later "flashing title bar on undecorated windows"
  fix; the window is not shown while the old style still has a caption.
- `update_theme` only redraws the title bar when `MARKER_DECORATIONS` is set,
  so a settings/theme change cannot paint a caption into a borderless window.
- `WM_NCACTIVATE` still updates the active/focused state and emits the same
  `Focused` events, but undecorated windows no longer fall through to
  `DefWindowProc`, which is what paints the caption.

The application additionally creates these windows hidden, swallows stray
`WM_NCPAINT` with a subclass guard (`src/borderless.rs`) and only then shows
them, so the first visible frame is already free of non-client pixels.

Run `node src-tauri/vendor/tao/run-regression.mjs` on Windows. It builds two
real windows, dirties the second one before the first one's paint is handled
and injects a synchronous `WM_PAINT` into the thread event target once the
nested dispatch is in progress, then reports `RESULT: survived`. Passing
`--control` builds the same probe against stock 0.35.3 and expects it to panic
with the production assertion, which is what keeps a passing run meaningful.
