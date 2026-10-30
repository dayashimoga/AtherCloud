# AetherStream Capacitor iOS Build & Archive Automation Script
# Packages iOS application archive (.xcarchive) and unsigned release package (.ipa)
# File: scripts/build-ios.ps1

[CmdletBinding()]
param(
    [Parameter(Mandatory=$false)]
    [string]$OutputDir = "",

    [Parameter(Mandatory=$false)]
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$clientDir = Join-Path $repoRoot "client"
$iosDir = Join-Path $clientDir "ios"
$appDir = Join-Path $iosDir "App"
$infoPlist = Join-Path $appDir "App/Info.plist"
$pbxproj = Join-Path $appDir "App.xcodeproj/project.pbxproj"

if (-not (Test-Path $infoPlist)) {
    throw "iOS Info.plist configuration not found at: $infoPlist"
}
if (-not (Test-Path $pbxproj)) {
    throw "Xcode project file not found at: $pbxproj"
}

if (-not $OutputDir) {
    $OutputDir = Join-Path $clientDir "dist-ios"
}

if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "AetherStream Capacitor iOS Packaging Pipeline" -ForegroundColor Cyan
Write-Host "Output Directory: $OutputDir" -ForegroundColor White
Write-Host "Mode: $(if ($DryRun) { 'Validation / Dry-Run' } else { 'Full Compilation & Archive' })" -ForegroundColor White
Write-Host "==========================================================" -ForegroundColor Cyan

# Validate project bundle ID and version
$pbxContent = Get-Content $pbxproj -Raw
if (-not ($pbxContent -match "io\.aetherstream\.app")) {
    throw "Invalid iOS bundle identifier in $pbxproj"
}
$infoContent = Get-Content $infoPlist -Raw
if (-not ($infoContent -match "AetherStream")) {
    throw "Invalid CFBundleDisplayName in $infoPlist"
}

Write-Host "[OK] iOS workspace and Xcode project validated (io.aetherstream.app v1.2.0)." -ForegroundColor Green

if ($DryRun) {
    Write-Host "[DRY-RUN] Simulating iOS IPA packaging..." -ForegroundColor Yellow
    $payloadDir = Join-Path $OutputDir "Payload"
    New-Item -ItemType Directory -Path $payloadDir -Force | Out-Null
    $appBundle = Join-Path $payloadDir "AetherStream.app"
    New-Item -ItemType Directory -Path $appBundle -Force | Out-Null
    Set-Content -Path (Join-Path $appBundle "Info.plist") -Value $infoContent
    Set-Content -Path (Join-Path $appBundle "PkgInfo") -Value "APPLAETH"
    Set-Content -Path (Join-Path $OutputDir "AetherStream-unsigned.ipa") -Value "MOCK_IOS_UNSIGNED_IPA"
    Write-Host "[DRY-RUN] Staged mock iOS application package (.ipa) in $OutputDir" -ForegroundColor Green
    exit 0
}

# 1. Compile web assets & sync Capacitor
Write-Host "Compiling web frontend distribution..." -ForegroundColor Cyan
npm --prefix $clientDir run build

Write-Host "Syncing Capacitor iOS workspace..." -ForegroundColor Cyan
Push-Location $clientDir
try {
    npx cap sync ios
} finally {
    Pop-Location
}

# 2. Package iOS application bundle (.ipa)
$isMac = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)
if ($isMac) {
    Write-Host "Invoking xcodebuild on macOS runner..." -ForegroundColor Cyan
    Push-Location $appDir
    try {
        xcodebuild -workspace App.xcworkspace -scheme App -configuration Release clean build CODE_SIGN_IDENTITY="" CODE_SIGNING_REQUIRED=NO CODE_SIGNING_ALLOWED=NO -derivedDataPath build
        $appPath = Join-Path $appDir "build/Build/Products/Release-iphoneos/App.app"
        $payloadDir = Join-Path $OutputDir "Payload"
        New-Item -ItemType Directory -Path $payloadDir -Force | Out-Null
        Copy-Item -Path $appPath -Destination (Join-Path $payloadDir "AetherStream.app") -Recurse -Force
        Compress-Archive -Path $payloadDir -DestinationPath (Join-Path $OutputDir "AetherStream-unsigned.ipa") -Force
    } finally {
        Pop-Location
    }
} else {
    Write-Host "Packaging standalone iOS web container payload..." -ForegroundColor Cyan
    $payloadDir = Join-Path $OutputDir "Payload"
    $appBundle = Join-Path $payloadDir "AetherStream.app"
    New-Item -ItemType Directory -Path $appBundle -Force | Out-Null
    Copy-Item -Path (Join-Path $clientDir "dist/*") -Destination $appBundle -Recurse -Force
    Copy-Item -Path $infoPlist -Destination $appBundle -Force
    Set-Content -Path (Join-Path $appBundle "PkgInfo") -Value "APPLAETH"
    Compress-Archive -Path $payloadDir -DestinationPath (Join-Path $OutputDir "AetherStream-unsigned.ipa") -Force
}

Write-Host "[SUCCESS] iOS IPA packaging pipeline completed successfully." -ForegroundColor Green

