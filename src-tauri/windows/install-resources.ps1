param(
  [Parameter(Mandatory=$true)][string]$Package,
  [Parameter(Mandatory=$true)][string]$InstallRoot,
  [Parameter(Mandatory=$true)][ValidateSet('fonts','stickers')][string]$PackId,
  [Parameter(Mandatory=$true)][string]$ExpectedHash,
  [Parameter(Mandatory=$true)][string]$ManifestPath
)
$ErrorActionPreference = 'Stop'
$stagePath = $null
$backupPath = $null
function Get-ResourceHash([string]$Path) {
  $stream = [IO.File]::OpenRead($Path)
  $algorithm = [Security.Cryptography.SHA256]::Create()
  try { return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '') }
  finally { $stream.Dispose(); $algorithm.Dispose() }
}
try {
  if ((Get-ResourceHash $Package) -ne $ExpectedHash) { throw 'Resource package checksum mismatch' }
  $specs = Get-Content -LiteralPath $ManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  $spec = $specs | Where-Object id -EQ $PackId
  if (!$spec) { throw 'Unknown resource pack' }
  $rootPath = [IO.Path]::GetFullPath($InstallRoot)
  $parentPath = [IO.Path]::GetFullPath((Join-Path $rootPath 'optional'))
  $packPath = [IO.Path]::GetFullPath((Join-Path $parentPath $PackId))
  if ([IO.Path]::GetDirectoryName($parentPath) -ne $rootPath.TrimEnd('\') -or [IO.Path]::GetDirectoryName($packPath) -ne $parentPath) { throw 'Invalid resource destination' }
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [IO.Compression.ZipFile]::OpenRead($Package)
  try {
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($entry in $archive.Entries) {
      if (($entry.FullName -ne 'pack.json' -and $entry.FullName -cnotin $spec.files) -or !$seen.Add($entry.FullName) -or $entry.Length -gt 10MB) { throw 'Unexpected archive entry' }
    }
    if ($seen.Count -ne $spec.files.Count + 1 -or !$seen.Contains('pack.json')) { throw 'Incomplete resource archive' }
  } finally { $archive.Dispose() }
  New-Item -ItemType Directory -Path $parentPath -Force | Out-Null
  $stagePath = Join-Path $parentPath ($PackId + '.staging-' + [Guid]::NewGuid())
  if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($stagePath)) -ne $parentPath) { throw 'Invalid staging path' }
  [IO.Compression.ZipFile]::ExtractToDirectory($Package, $stagePath)
  $manifest = Get-Content -LiteralPath (Join-Path $stagePath 'pack.json') -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($manifest.id -ne $PackId -or $manifest.version -ne $spec.version) { throw 'Resource version mismatch' }
  foreach ($file in $spec.files) {
    $expected = $manifest.files.PSObject.Properties[$file].Value
    if (!$expected -or (Get-ResourceHash (Join-Path $stagePath $file)) -ne $expected) { throw 'Resource file checksum mismatch' }
  }
  if (Test-Path -LiteralPath $packPath) {
    $backupPath = Join-Path $parentPath ($PackId + '.backup-' + [Guid]::NewGuid())
    if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($backupPath)) -ne $parentPath) { throw 'Invalid backup path' }
    Move-Item -LiteralPath $packPath -Destination $backupPath
  }
  try { Move-Item -LiteralPath $stagePath -Destination $packPath } catch {
    if ($backupPath) { Move-Item -LiteralPath $backupPath -Destination $packPath; $backupPath = $null }
    throw
  }
  $stagePath = $null
  # Every recursive removal is restricted to a verified direct child of optional/.
  if ($backupPath) {
    if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($backupPath)) -ne $parentPath) { throw 'Invalid backup path' }
    Remove-Item -LiteralPath $backupPath -Recurse -Force
  }
  exit 0
} catch {
  Write-Error $_ -ErrorAction Continue
  if ($stagePath -and (Test-Path -LiteralPath $stagePath)) {
    if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($stagePath)) -eq $parentPath) { Remove-Item -LiteralPath $stagePath -Recurse -Force }
  }
  exit 1
}
