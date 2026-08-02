# Load MSVC environment, then run the Tauri desktop edge app.
# Fixes LNK1104 msvcrt.lib / missing excpt.h when a bare PowerShell
# finds link.exe without LIB/INCLUDE (common with mixed VS 18 + BuildTools).
#
# Usage:
#   ./scripts/tauri-dev.ps1
#   ./scripts/tauri-dev.ps1 -- build
#
$ErrorActionPreference = "Stop"

function Find-VsDevCmd {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $install = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null
        if ($install) {
            $candidate = Join-Path $install "Common7\Tools\VsDevCmd.bat"
            if (Test-Path $candidate) { return $candidate }
        }
    }
    $fallbacks = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat",
        "${env:ProgramFiles}\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat"
    )
    foreach ($p in $fallbacks) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

$vsDev = Find-VsDevCmd
if (-not $vsDev) {
    Write-Error @"
Could not find VsDevCmd.bat. Install Visual Studio Build Tools with the
'Desktop development with C++' workload (MSVC + Windows SDK), then retry.
"@
}

$repo = Split-Path -Parent $PSScriptRoot
$edge = Join-Path $repo "desktop-edge"
if (-not (Test-Path (Join-Path $edge "package.json"))) {
    Write-Error "desktop-edge/package.json not found at $edge"
}

$npmArgs = if ($args.Count -gt 0) { $args } else { @("run", "tauri", "dev") }
# Join for cmd.exe; quote each arg that has spaces
$npmArgLine = ($npmArgs | ForEach-Object {
    if ($_ -match '\s') { '"{0}"' -f $_ } else { $_ }
}) -join " "

Write-Host "Using VS env: $vsDev"
Write-Host "npm.cmd $npmArgLine  (cwd=$edge)"

# Import VsDevCmd into this cmd session, then npm. Prefer BuildTools-complete env.
$cmd = @"
call "$vsDev" -arch=amd64 -host_arch=amd64 && cd /d "$edge" && npm.cmd $npmArgLine
"@
cmd /c $cmd
exit $LASTEXITCODE
