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

Resize handling: WM_SIZE updates both the controller bounds and its separate
WRY_WEBVIEW container HWND. Updating only the controller leaves the container
at its creation size, clipping fullscreen content to the top-left rectangle.
SetWindowPos is guarded by `container != hwnd`: resizing the HWND currently
dispatching WM_SIZE would recurse and cause 0xc00000fd. The native regression
resizes a live WebView2 parent repeatedly and checks both container and
controller dimensions, as well as teardown safety.
