param(
    [string]$Version = "0.1.0",
    [switch]$PerfLog,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent
& python (Join-Path $repo "scripts\generate-third-party.py") --verify-release --scope desktop
if ($LASTEXITCODE -ne 0) { throw "Third-party license verification failed." }

$sourceUrl = if ($env:ATLAS_SOURCE_URL) { $env:ATLAS_SOURCE_URL } else {
    "https://github.com/neondeex/atlas-filemanager/releases/download/v$Version/atlas-$Version-source.zip"
}
$installerDirectory = Join-Path $repo 'target\installer'
New-Item -ItemType Directory -Force -Path $installerDirectory | Out-Null
@"
Atlas $Version corresponding source and build instructions:
$sourceUrl
The matching atlas-$Version-source.zip accompanies this test distribution.
Atlas: AGPL-3.0-or-later. Third-party components retain their own licenses.
See RCLONE-THIRD-PARTY-NOTICES.txt for the bundled rclone dependency notices.
"@ | Set-Content -LiteralPath (Join-Path $installerDirectory 'SOURCE-CODE.txt') -Encoding UTF8

if (-not $SkipBuild) {
if ($PerfLog) {
    cargo build --release --locked -p atlas -p atlas-search-service -p atlas-cloud-extension -p atlas-icon-extension --features perf-log
} else {
    cargo build --release --locked -p atlas -p atlas-search-service -p atlas-cloud-extension -p atlas-icon-extension
}
if ($LASTEXITCODE -ne 0) { throw "Windows build failed ($LASTEXITCODE)." }
}
& (Join-Path $PSScriptRoot "archive-symbols.ps1") -Version "$Version-installer"
# The Windows installer carries the same complete payload as the Cloud package.
& (Join-Path $PSScriptRoot "build-cloud-extension.ps1") -SkipBuild
& (Join-Path $PSScriptRoot "stage-windows-runtime.ps1")

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
$isccPath = if ($iscc) { $iscc.Source } else { $null }
if (-not $isccPath) {
    $commonCandidates = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
    )
    foreach ($cand in $commonCandidates) {
        if (Test-Path $cand) {
            $isccPath = $cand
            break
        }
    }
    if (-not $isccPath) {
        throw "Inno Setup compiler not found. Install Inno Setup, then run this again."
    }
}

& $isccPath "/DAppVersion=$Version" "packaging\windows\atlas.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)." }

$installer = "target\installer\AtlasSetup-$Version.exe"
if (-not (Test-Path $installer)) { throw "Installer was not produced: $installer" }
if (Test-Path $installer) {
    Get-FileHash $installer -Algorithm SHA256
}
