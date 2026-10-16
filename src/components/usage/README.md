# Usage and context insights

Inspired by the activity, cache and context composition views in
[dsh-context](https://github.com/bowenliang123/dsh-context). This is a native
integration with Vivian's existing usage store and prompt inspector; the plugin
is not installed or executed.

- API usage remains provider-reported data. Input, cache read, cache write and
  output are disjoint counters. Cache share uses all input categories as its
  denominator. No monetary prices are inferred.
- The activity calendar always requests at least 56 days. Selecting a date
  overrides the chosen range across totals, charts and dimensions. Clearing the
  selection restores that range. Missing historical attribution remains a
  residual row rather than being reconstructed from current settings.
- Prompt snapshots are the latest actual assembly cached in memory for each
  running character. Metadata is listed separately; original content is loaded
  on selection. Templates are not substituted when snapshots are absent.
- Snapshot section tokens are estimates, not API usage or complete context
  window occupancy. Extra request parameters are shown separately. The view
  does not persist prompts, create a historical timeline or track compaction.
- Snapshot requests discard stale responses after character changes or unmount.
  Original content is rendered as plain text.

Validation: `node tests/usage-dashboard.test.mjs`, `npm run typecheck`, Vite build,
and `cargo check --locked --offline --manifest-path src-tauri/Cargo.toml --lib -j1`.
UI states are checked using a local preview with clearly labeled sample data.
