param(
  [Parameter(Mandatory=$true)][string]$Package,
  [Parameter(Mandatory=$true)][string]$Destination,
  [Parameter(Mandatory=$true)][string]$ExpectedHash
)
$ErrorActionPreference = 'Stop'
$stagePath = $null
$backupPath = $null
function Get-ApartmentHash([string]$Path) {
  $stream = [IO.File]::OpenRead($Path)
  $algorithm = [Security.Cryptography.SHA256]::Create()
  try { return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '') }
  finally { $stream.Dispose(); $algorithm.Dispose() }
}
try {
  if ((Get-ApartmentHash $Package) -ne $ExpectedHash) {
    throw 'Apartment package checksum mismatch'
  }
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [IO.Compression.ZipFile]::OpenRead($Package)
  try {
    foreach ($entry in $archive.Entries) {
      if ($entry.FullName -notmatch '^(plugin\.json|ui/room\.js|room/[a-zA-Z0-9_/-]+\.glb)$' -or $entry.FullName.Contains('..')) {
        throw 'Unexpected entry in apartment package'
      }
    }
  } finally { $archive.Dispose() }
  $pluginPath = [IO.Path]::GetFullPath($Destination)
  if ([IO.Path]::GetFileName($pluginPath) -ne '3d-apartment') { throw 'Invalid plugin destination' }
  $parentPath = [IO.Path]::GetDirectoryName($pluginPath)
  New-Item -ItemType Directory -Path $parentPath -Force | Out-Null
  $stagePath = Join-Path $parentPath ('3d-apartment.staging-' + [Guid]::NewGuid())
  [IO.Compression.ZipFile]::ExtractToDirectory($Package, $stagePath)
  $manifest = Get-Content -LiteralPath (Join-Path $stagePath 'plugin.json') -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($manifest.name -ne '3d-apartment' -or !(Test-Path -LiteralPath (Join-Path $stagePath 'ui/room.js'))) {
    throw 'Incomplete apartment package'
  }
  if (Test-Path -LiteralPath $pluginPath) {
    $backupPath = Join-Path $parentPath ('3d-apartment.backup-' + [Guid]::NewGuid())
    Move-Item -LiteralPath $pluginPath -Destination $backupPath
  }
  try { Move-Item -LiteralPath $stagePath -Destination $pluginPath } catch {
    if ($backupPath) { Move-Item -LiteralPath $backupPath -Destination $pluginPath; $backupPath = $null }
    throw
  }
  $stagePath = $null
  if ($backupPath) { Remove-Item -LiteralPath $backupPath -Recurse -Force }
  exit 0
} catch {
  Write-Error $_ -ErrorAction Continue
  if ($stagePath -and (Test-Path -LiteralPath $stagePath)) { Remove-Item -LiteralPath $stagePath -Recurse -Force }
  exit 1
}
