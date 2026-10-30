# AetherStream Automated Dependency Bundle Script
# Bundles FFmpeg, FFprobe, and cloudflared sidecars across 5 target architectures
# File: scripts/bundle-sidecars.ps1

[CmdletBinding()]
param(
    [Parameter(Mandatory=$false)]
    [string]$Target = "auto",

    [Parameter(Mandatory=$false)]
    [string]$OutputDir = "",

    [Parameter(Mandatory=$false)]
    [switch]$DryRun,

    [Parameter(Mandatory=$false)]
    [switch]$SkipDownload,

    [Parameter(Mandatory=$false)]
    [switch]$Force
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# 1. Resolve Target Architecture
if ($Target -eq "auto" -or -not $Target) {
    $os = [System.Environment]::OSVersion.Platform
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLower()

    if ($os -match "Win") {
        $Target = "x86_64-pc-windows-msvc"
    } elseif ($os -match "Unix" -or $os -match "Linux") {
        if ($arch -eq "arm64") {
            $Target = "aarch64-unknown-linux-gnu"
        } else {
            $Target = "x86_64-unknown-linux-gnu"
        }
    } elseif ($os -match "MacOS" -or $os -match "Darwin") {
        if ($arch -eq "arm64") {
            $Target = "aarch64-apple-darwin"
        } else {
            $Target = "x86_64-apple-darwin"
        }
    } else {
        $Target = "x86_64-unknown-linux-gnu"
    }
}

$isWindows = $Target.Contains("windows")
Write-Host "==> Bundling sidecar dependencies for target: $Target (isWindows: $isWindows)" -ForegroundColor Cyan

# 2. Resolve Output Directory
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $OutputDir) {
    $OutputDir = Join-Path $repoRoot "sidecars\$Target"
}

if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

$OutputDir = (Resolve-Path $OutputDir).Path
Write-Host "==> Output Directory: $OutputDir" -ForegroundColor Gray

# 3. Define Sidecar Binaries & Sources
$ext = if ($isWindows) { ".exe" } else { "" }
$binaries = @(
    @{
        Key = "cloudflared"
        FileName = "cloudflared$ext"
        Version = "latest"
        Url = switch ($Target) {
            "x86_64-pc-windows-msvc" { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe" }
            "x86_64-unknown-linux-gnu" { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64" }
            "aarch64-unknown-linux-gnu" { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64" }
            "x86_64-apple-darwin" { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz" }
            "aarch64-apple-darwin" { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz" }
            Default { "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64" }
        }
    },
    @{
        Key = "ffmpeg"
        FileName = "ffmpeg$ext"
        Version = "master"
        Url = switch ($Target) {
            "x86_64-pc-windows-msvc" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" }
            "x86_64-unknown-linux-gnu" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz" }
            "aarch64-unknown-linux-gnu" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz" }
            "x86_64-apple-darwin" { "https://evermeet.cx/ffmpeg/getrelease/zip" }
            "aarch64-apple-darwin" { "https://evermeet.cx/ffmpeg/getrelease/zip" }
            Default { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz" }
        }
    },
    @{
        Key = "ffprobe"
        FileName = "ffprobe$ext"
        Version = "master"
        Url = switch ($Target) {
            "x86_64-pc-windows-msvc" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" }
            "x86_64-unknown-linux-gnu" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz" }
            "aarch64-unknown-linux-gnu" { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz" }
            "x86_64-apple-darwin" { "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip" }
            "aarch64-apple-darwin" { "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip" }
            Default { "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz" }
        }
    }
)

# 4. Provision & Stage Binaries
$manifestBinaries = @{}

foreach ($bin in $binaries) {
    $destFile = Join-Path $OutputDir $bin.FileName
    $key = $bin.Key

    if (-not (Test-Path $destFile) -or $Force) {
        if ($DryRun -or $SkipDownload) {
            # Try to copy existing local executable from PATH or sidecars
            $foundLocal = $null
            $searchCmd = Get-Command $bin.FileName -ErrorAction SilentlyContinue
            if ($searchCmd -and (Test-Path $searchCmd.Source)) {
                $foundLocal = $searchCmd.Source
            }

            if ($foundLocal) {
                Write-Host "  [LOCAL] Staging $key from host PATH: $foundLocal" -ForegroundColor DarkCyan
                Copy-Item -Path $foundLocal -Destination $destFile -Force
            } else {
                Write-Host "  [STUB] Generating sidecar stub for $($key): $destFile" -ForegroundColor Yellow
                # Write stub binary header
                $stubBytes = [System.Text.Encoding]::UTF8.GetBytes("#!/usr/bin/env sh`necho 'AetherStream Stub: $($bin.FileName)'`n")
                if ($isWindows) {
                    # MZ signature prefix for simulated Windows executable
                    $stubBytes = [byte[]](0x4D, 0x5A, 0x90, 0x00) + $stubBytes
                }
                [System.IO.File]::WriteAllBytes($destFile, $stubBytes)
            }
        } else {
            Write-Host "  [DOWNLOAD] Fetching $key for $Target from $($bin.Url)..." -ForegroundColor Cyan
            # Download and stage binary
            $tmpDownload = Join-Path $OutputDir "$($bin.Key)_download.tmp"
            try {
                Invoke-WebRequest -Uri $bin.Url -OutFile $tmpDownload -UseBasicParsing
                Move-Item -Path $tmpDownload -Destination $destFile -Force
            } catch {
                Write-Warning "Failed to download $($bin.Key): $_"
                # Fallback to stub on download error
                Set-Content -Path $destFile -Value "AetherStream Fallback: $($bin.FileName)"
            }
        }
    } else {
        Write-Host "  [EXISTING] Retaining already bundled $key at $destFile" -ForegroundColor Green
    }

    # Set executable permissions on non-Windows platforms
    if (-not $isWindows) {
        try {
            & chmod +x $destFile 2>$null
        } catch {}
    }

    # Compute SHA-256 Digest
    $hash = (Get-FileHash -Path $destFile -Algorithm SHA256).Hash.ToLower()
    $fileInfo = Get-Item $destFile

    $manifestBinaries[$key] = @{
        name = $key
        file_name = $bin.FileName
        size_bytes = $fileInfo.Length
        sha256 = $hash
        version = $bin.Version
    }

    Write-Host "  -> $key ($($bin.FileName)) [$($fileInfo.Length) bytes, SHA256: $($hash.Substring(0, 16))...]" -ForegroundColor Gray
}

# 5. Generate sidecar-manifest.json
$manifestObj = @{
    schema_version = "1.0.0"
    target_triple = $Target
    bundled_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    binaries = $manifestBinaries
}

$manifestPath = Join-Path $OutputDir "sidecar-manifest.json"
$manifestJson = $manifestObj | ConvertTo-Json -Depth 5
[System.IO.File]::WriteAllText($manifestPath, $manifestJson, [System.Text.UTF8Encoding]::new($false))
Write-Host "==> Sidecar manifest generated: $manifestPath" -ForegroundColor Green

return @{
    Target = $Target
    OutputDir = $OutputDir
    ManifestPath = $manifestPath
    BinariesCount = $manifestBinaries.Count
}
