<#
.SYNOPSIS
  Builds NabyTune (release) and wraps it in target\package\NabyTune-Setup.exe.

.DESCRIPTION
  1. cargo build --release for both bins: the app (nabytune-gpui-proto.exe) and naby-login.exe.
  2. Stages the payload in target\package\payload: NabyTune.exe, naby-login.exe, nabytune.ico.
  3. Builds the installer crate (installer\, its own Cargo.lock and target dir) with
     NABY_PAYLOAD_DIR pointing at that payload, and copies the result to
     target\package\NabyTune-Setup.exe.
  4. Writes target\package\NabyTune-Setup.exe.sha256 (the lowercase hex SHA-256, nothing else)
     beside it. Publish both on a GitHub release tagged v<version>: that is what the app's
     updater (src\updater.rs) looks for.

  The installer needs no external tools beyond the Rust toolchain; the Windows SDK's rc.exe, if
  present, gives it an icon, a comctl32 v6 manifest (task dialogs) and version info.

.PARAMETER SkipAppBuild
  Package the binaries already in target\release instead of building them.

.PARAMETER Version
  The version to show and register (default: Cargo.toml's).
#>
[CmdletBinding()]
param(
    [switch]$SkipAppBuild,
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$release = Join-Path $root 'target\release'
$package = Join-Path $root 'target\package'
$payload = Join-Path $package 'payload'
$installer = Join-Path $root 'installer'

function Invoke-Cargo {
    param([string[]]$CargoArgs)
    Write-Host "> cargo $($CargoArgs -join ' ')" -ForegroundColor Cyan
    # Cargo reports progress on stderr; Windows PowerShell turns that into errors under 'Stop'
    # once output is redirected, so only its exit code decides.
    $ErrorActionPreference = 'Continue'
    & cargo @CargoArgs 2>&1 | ForEach-Object { "$_" }
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw "cargo $($CargoArgs -join ' ') failed (exit $LASTEXITCODE)" }
}

# 1. The app and its sign-in helper.
if (-not $SkipAppBuild) {
    Invoke-Cargo @('build', '--release', '--bins', '--manifest-path', (Join-Path $root 'Cargo.toml'))
}
$app = Join-Path $release 'nabytune-gpui-proto.exe'
$login = Join-Path $release 'naby-login.exe'
foreach ($f in $app, $login) {
    if (-not (Test-Path $f)) { throw "missing $f (build gpui-proto in release first, or drop -SkipAppBuild)" }
}

# 1b. The app must not carry this machine's project folder: anything it reads from there at run
#     time works here and nowhere else (0.1.0 read its seed songs from it, and wouldn't start).
foreach ($f in $app, $login) {
    if (Select-String -Path $f -Pattern $root -SimpleMatch -Quiet) {
        throw "$f contains the build folder ($root): something reads a build-time path at run time"
    }
}

# 2. Payload.
if (Test-Path $payload) { Remove-Item -Recurse -Force $payload }
New-Item -ItemType Directory -Force $payload | Out-Null
Copy-Item $app (Join-Path $payload 'NabyTune.exe')
Copy-Item $login (Join-Path $payload 'naby-login.exe')
$icon = Join-Path $root 'assets\nabytune.ico'
if (Test-Path $icon) {
    Copy-Item $icon (Join-Path $payload 'nabytune.ico')
} else {
    Write-Warning "no ${icon}: shortcuts, Installed apps and the setup exe will have no NabyTune icon"
}

# 3. The installer.
$env:NABY_PAYLOAD_DIR = $payload
if ($Version) { $env:NABY_VERSION = $Version } else { Remove-Item Env:NABY_VERSION -ErrorAction SilentlyContinue }
$targetDir = Join-Path $installer 'target'
Invoke-Cargo @('build', '--release', '--manifest-path', (Join-Path $installer 'Cargo.toml'), '--target-dir', $targetDir)

$setup = Join-Path $package 'NabyTune-Setup.exe'
Copy-Item (Join-Path $targetDir 'release\NabyTune-Setup.exe') $setup -Force
$mb = '{0:N1}' -f ((Get-Item $setup).Length / 1MB)
Write-Host "Built $setup ($mb MB)" -ForegroundColor Green

# 4. Its checksum, for the app's updater (which also accepts GitHub's own asset digest).
$hash = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLowerInvariant()
$sumFile = "$setup.sha256"
[System.IO.File]::WriteAllText($sumFile, $hash)
Write-Host "SHA-256 $hash -> $sumFile" -ForegroundColor Green

# 5. A portable copy, as a zip: no installer to run, and browsers rarely hold a zip back the way
#    they do an unsigned .exe nobody has downloaded before.
$portable = Join-Path $package 'NabyTune-portable.zip'
if (Test-Path $portable) { Remove-Item -Force $portable }
Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $portable
$zipMb = '{0:N1}' -f ((Get-Item $portable).Length / 1MB)
Write-Host "Built $portable ($zipMb MB)" -ForegroundColor Green

$shown = $Version
if (-not $shown) {
    $m = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $shown = if ($m) { $m.Matches[0].Groups[1].Value } else { '<version>' }
}
Write-Host "Publish: gh release create v$shown `"$setup`" `"$sumFile`" `"$portable`" --title `"NabyTune $shown`" --notes `"...`""
