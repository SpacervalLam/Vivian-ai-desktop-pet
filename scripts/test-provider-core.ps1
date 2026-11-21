param([string]$Filter = '')
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
# Let Cargo select one coherent dependency graph; newest-file selection can mix
# incompatible serde/serde_json artifacts when other builds share target/.
$manifest = Join-Path $workspace 'src-tauri/provider-contracts/Cargo.toml'
$target = Join-Path $workspace 'src-tauri/target'
& cargo test --offline --locked --manifest-path $manifest --target-dir $target -j 1 --lib $Filter -- --test-threads=1
exit $LASTEXITCODE
