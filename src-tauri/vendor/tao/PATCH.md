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

Run `node src-tauri/vendor/tao/run-regression.mjs` on Windows. It builds two
real windows, dirties the second one before the first one's paint is handled
and injects a synchronous `WM_PAINT` into the thread event target once the
nested dispatch is in progress, then reports `RESULT: survived`. Passing
`--control` builds the same probe against stock 0.35.3 and expects it to panic
with the production assertion, which is what keeps a passing run meaningful.
