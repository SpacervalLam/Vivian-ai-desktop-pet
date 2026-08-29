$ErrorActionPreference = 'Stop'

$files = @(
    "$env:APPDATA\Vivian\characters\vivian\sound\config.json",
    "$env:APPDATA\Vivian\characters\nana\sound\config.json"
)

foreach ($f in $files) {
    if (-not (Test-Path $f)) { Write-Host "SKIP (not found): $f"; continue }

    $json = Get-Content -Raw -Encoding UTF8 $f | ConvertFrom-Json

    if ($null -ne $json.gpt_sovits_ref_audio) {
        $main = ($json.gpt_sovits_ref_audio | Out-String).Trim()
        if ($main) {
            Write-Host "CLEAR main: $main"
            $json.gpt_sovits_ref_audio = $null
        }
    }

    if ($null -ne $json.PSObject.Properties['gpt_sovits_aux_ref_audios']) {
        Write-Host "DROP whole aux_ref_audios list"
        $json.PSObject.Properties.Remove('gpt_sovits_aux_ref_audios')
    }

    $json | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 $f
    Write-Host "SAVED: $f"
}