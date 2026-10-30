# AetherStream Automated Release Notes Generator
# Generates structured GitHub Release notes with feature highlights and SHA-256 digest manifest
# File: scripts/generate-release-notes.ps1

[CmdletBinding()]
param(
    [Parameter(Mandatory=$false)]
    [string]$Version = "v1.2.0",

    [Parameter(Mandatory=$false)]
    [string]$OutputPath = "",

    [Parameter(Mandatory=$false)]
    [string]$ChecksumFile = "",

    [Parameter(Mandatory=$false)]
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "RELEASE_NOTES.md"
}

if (-not $ChecksumFile) {
    $ChecksumFile = Join-Path $repoRoot "release-artifacts/SHA256SUMS.txt"
}

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "AetherStream Release Notes Generator" -ForegroundColor Cyan
Write-Host "Target Version: $Version" -ForegroundColor White
Write-Host "Output File: $OutputPath" -ForegroundColor White
Write-Host "Mode: $(if ($DryRun) { 'Validation / Dry-Run' } else { 'Generate & Write' })" -ForegroundColor White
Write-Host "==========================================================" -ForegroundColor Cyan

$lines = @(
    "## AetherStream $Version Production Release",
    "",
    "AetherStream is an enterprise-grade, high-performance personal cloud media streaming and storage orchestrator built in Rust and modern TypeScript.",
    "",
    "### Key Architectural Pillars",
    "- **Multi-Architecture Native Daemon**: Linux x86_64, Linux ARM64, Windows x64, macOS Intel & Apple Silicon.",
    "- **Virtual Storage Bus & Hotplug Monitoring**: Real-time OS storage discovery, uevents/disk arbitration, SQLite WAL metadata indexing.",
    "- **Hardware Transcoding & ABR Engine**: Automated ladder (1080p, 720p, 480p, 360p) with NVENC, QSV, VAAPI, VideoToolbox.",
    "- **Zero-Cost Remote Ingress**: Embedded Cloudflare Tunnel daemon supervisor with instant QR-code pairing and HMAC-SHA256 access tokens.",
    "- **Universal Web Player & PWA**: Adaptive HLS player, pinch-to-zoom touch controls, offline app shell service worker caching.",
    "- **Cross-Platform Native Clients**: Tauri desktop (.msi, .exe, .deb, .AppImage, .dmg), Capacitor Android (.apk, .aab), iOS (.ipa).",
    "",
    "### Distribution Packages",
    "| Platform | Package Format | Target Architecture |",
    "| :--- | :--- | :--- |",
    "| Linux (Server) | Standalone Binary | ``x86_64-unknown-linux-gnu``, ``aarch64-unknown-linux-gnu`` |",
    "| Windows (Server) | Standalone Binary (`.exe`) | ``x86_64-pc-windows-msvc`` |",
    "| macOS (Server) | Standalone Binary | ``x86_64-apple-darwin``, ``aarch64-apple-darwin`` |",
    "| Windows (Desktop) | WiX MSI (`.msi`) & NSIS (`.exe`) | ``x86_64`` |",
    "| Linux (Desktop) | Debian (`.deb`) & AppImage (`.AppImage`) | ``x86_64`` |",
    "| macOS (Desktop) | Disk Image (`.dmg`) & App Bundle | ``universal`` / ``x86_64`` / ``arm64`` |",
    "| Android | Debug APK, Signed Release APK (`.apk`), Play Store (`.aab`) | Universal ARM64/x86 |",
    "| iOS | Application Package (`.ipa`) & Xcode Archive | Universal iOS Device |",
    "",
    "### Cryptographic Checksums (SHA-256)",
    '```text'
)

if (Test-Path $ChecksumFile) {
    $checksums = Get-Content $ChecksumFile
    $lines += $checksums
} else {
    $lines += "# Verification Checksums generated during CI release compilation."
    $lines += "SHA256SUMS.txt will accompany the release bundle."
}

$lines += '```'
$lines += ""
$lines += "---"
$lines += "*Published automatically by AetherStream Release CI/CD Automation.*"

$resultNotes = $lines -join "`n"

if ($DryRun) {
    Write-Host "[DRY-RUN] Release notes successfully generated in memory." -ForegroundColor Green
    Write-Host $resultNotes -ForegroundColor Gray
    exit 0
}

[System.IO.File]::WriteAllText($OutputPath, $resultNotes, [System.Text.Encoding]::UTF8)
Write-Host "[SUCCESS] Release notes written to: $OutputPath" -ForegroundColor Green
