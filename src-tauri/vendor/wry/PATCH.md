# Windows parent subclass teardown fix

Vendored from crates.io wry 0.55.1, with its original MIT/Apache-2.0 licenses.

The application's crash log points to `parent_subclass_proc` dereferencing a
null controller in `WM_SETFOCUS`. Version 0.55.1 releases the controller and
leaves the subclass installed with null reference data during destruction.
Focus, move and size callbacks still dereference that reference data.

Backport the teardown ordering in upstream
https://github.com/tauri-apps/wry/blob/dev/src/webview2/mod.rs : remove the
parent subclass before releasing its COM controller. Also forward messages
with null reference data directly to `DefSubclassProc`.

The patch is selected by `[patch.crates-io]` in the application manifest; no
machine-local Cargo registry files are modified. Remove the override after
upgrading to an upstream release containing the fix and verifying teardown.

Run `node src-tauri/vendor/wry/run-regression.mjs` on Windows. It tests null
controller messages on a real HWND and ten real WebView2 lifecycle cycles
(both parent-first and controller-first), including asserting the parent
subclass was removed. The temporary
manifest reuses the application lockfile and excludes upstream GPU examples.

Screenshot overlay resize fix: WM_SIZE updates only the controller bounds.
Calling SetWindowPos on Controller.ParentWindow from this callback resizes
the same parent and recursively dispatches WM_SIZE, causing 0xc00000fd.
The native regression now resizes a live WebView2 parent repeatedly.
