# AetherStream Desktop Packaging Automation Script
# Packages Tauri desktop clients across Windows (.msi/.exe), Linux (.deb/.AppImage), macOS (.dmg)
# File: scripts/build-desktop.ps1

[CmdletBinding()]
param(
    [Parameter(Mandatory=$false)]
    [ValidateSet("windows", "linux", "macos", "auto")]
    [string]$Platform = "auto",

    [Parameter(Mandatory=$false)]
    [string]$OutputDir = "",

    [Parameter(Mandatory=$false)]
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$clientDir = Join-Path $repoRoot "client"
$tauriConfPath = Join-Path $clientDir "src-tauri/tauri.conf.json"

if (-not (Test-Path $tauriConfPath)) {
    throw "Tauri configuration not found at $tauriConfPath"
}

if (-not $OutputDir) {
    $OutputDir = Join-Path $clientDir "dist-desktop"
}

if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

# Resolve platform if auto
if ($Platform -eq "auto") {
    $isWin = [System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT
    $isMac = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)
    if ($isWin) {
        $Platform = "windows"
    } elseif ($isMac) {
        $Platform = "macos"
    } else {
        $Platform = "linux"
    }
}

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "AetherStream Desktop Bundler" -ForegroundColor Cyan
Write-Host "Target Platform: $Platform" -ForegroundColor White
Write-Host "Output Directory: $OutputDir" -ForegroundColor White
Write-Host "Mode: $(if ($DryRun) { 'Validation / Dry-Run' } else { 'Full Release Build' })" -ForegroundColor White
Write-Host "==========================================================" -ForegroundColor Cyan

# Validate tauri.conf.json
$confRaw = Get-Content $tauriConfPath -Raw
$conf = $confRaw | ConvertFrom-Json

if (-not $conf.tauri.bundle.active) {
    throw "Tauri bundle configuration is not active!"
}

$targets = $conf.tauri.bundle.targets
Write-Host "[OK] Tauri bundle targets: $($targets -join ', ')" -ForegroundColor Green

if ($DryRun) {
    Write-Host "[DRY-RUN] Simulating build outputs for platform: $Platform" -ForegroundColor Yellow
    switch ($Platform) {
        "windows" {
            Set-Content -Path (Join-Path $OutputDir "AetherStream.msi") -Value "MOCK_TAURI_MSI_PACKAGE"
            Set-Content -Path (Join-Path $OutputDir "AetherStream-Setup.exe") -Value "MOCK_TAURI_NSIS_PACKAGE"
        }
        "linux" {
            Set-Content -Path (Join-Path $OutputDir "aetherstream_amd64.deb") -Value "MOCK_TAURI_DEB_PACKAGE"
            Set-Content -Path (Join-Path $OutputDir "aetherstream.AppImage") -Value "MOCK_TAURI_APPIMAGE_PACKAGE"
        }
        "macos" {
            Set-Content -Path (Join-Path $OutputDir "AetherStream.dmg") -Value "MOCK_TAURI_DMG_PACKAGE"
        }
    }
    Write-Host "[DRY-RUN] Mock packages staged in $OutputDir" -ForegroundColor Green
    exit 0
}

# Real build execution
Write-Host "Building client web distribution..." -ForegroundColor Cyan
npm --prefix $clientDir run build

Write-Host "Invoking Tauri bundler..." -ForegroundColor Cyan
npm --prefix $clientDir run tauri:build

Write-Host "Collecting packaged artifacts..." -ForegroundColor Cyan
$bundleDir = Join-Path $clientDir "src-tauri/target/release/bundle"
if (Test-Path $bundleDir) {
    Get-ChildItem -Path $bundleDir -Recurse -File | Where-Object {
        $_.Extension -in @(".msi", ".exe", ".deb", ".AppImage", ".dmg", ".app")
    } | ForEach-Object {
        Copy-Item -Path $_.FullName -Destination $OutputDir -Force
        Write-Host "Packaged: $($_.Name)" -ForegroundColor Green
    }
}

Write-Host "[SUCCESS] Desktop bundling completed successfully." -ForegroundColor Green
