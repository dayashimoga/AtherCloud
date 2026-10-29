# AetherStream Master Orchestrator & Bootstrapping Engine
# File: bootstrap.ps1
# Multi-Platform: Windows PowerShell 5.1 & PowerShell 7+ on Linux/macOS/Windows

[CmdletBinding(DefaultParameterSetName="Default")]
param(
    [Parameter(ParameterSetName="Default")]
    [switch]$Interactive,

    [Parameter(ParameterSetName="Default")]
    [switch]$RunNext,

    [Parameter(ParameterSetName="Default")]
    [switch]$Status,

    [Parameter(ParameterSetName="Manual")]
    [int]$Stage,

    [Parameter(ParameterSetName="Manual")]
    [string]$Phase,

    [Parameter(ParameterSetName="Manual")]
    [string]$Subphase,

    [Parameter(Mandatory=$false)]
    [switch]$AutoApprove,

    [Parameter(Mandatory=$false)]
    [switch]$ValidateOnly,

    [Parameter(Mandatory=$false)]
    [switch]$InitRepo
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Script:RepoRoot = $PSScriptRoot
$commonScript = Join-Path $Script:RepoRoot "scripts/common.ps1"
if (-not (Test-Path $commonScript)) {
    throw "Fatal: Missing common automation library at: $commonScript"
}
. $commonScript

Write-Host "================================================================================" -ForegroundColor Cyan
Write-Host "             AETHERSTREAM PRODUCTION BOOTSTRAP ORCHESTRATOR                     " -ForegroundColor Yellow
Write-Host "================================================================================" -ForegroundColor Cyan

# 1. Environment Diagnostics & Toolchain Verification
Write-AetherLog "STEP" "PHASE 1.1: AUDITING HOST ENVIRONMENT & TOOLCHAINS"

$isElevated = $false
if ([System.Environment]::OSVersion.Platform -like "*Win*") {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $isElevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
Write-AetherLog "INFO" "PowerShell Version: $($PSVersionTable.PSVersion) | Admin Elevation: $isElevated"

# Check CPU & RAM
$cpuCores = [System.Environment]::ProcessorCount
Write-AetherLog "INFO" "Logical CPU Cores Detected: $cpuCores"

$tools = @(
    @{ Name = "git"; Required = $true },
    @{ Name = "cargo"; Required = $true },
    @{ Name = "rustc"; Required = $true },
    @{ Name = "node"; Required = $true },
    @{ Name = "npm"; Required = $true },
    @{ Name = "ffmpeg"; Required = $false },
    @{ Name = "gh"; Required = $false }
)

$missingRequired = @()
foreach ($t in $tools) {
    $found = Get-Command $t.Name -ErrorAction SilentlyContinue
    if ($found) {
        Write-AetherLog "SUCCESS" "Tool verified: $($t.Name) -> $($found.Source)"
    } else {
        if ($t.Required) {
            Write-AetherLog "ERROR" "CRITICAL: Required toolchain binary missing: $($t.Name)"
            $missingRequired += $t.Name
        } else {
            Write-AetherLog "WARN" "Optional tooling not detected on host PATH: $($t.Name)"
        }
    }
}

if ($missingRequired.Count -gt 0) {
    throw "Halting bootstrap: Missing required toolchains: $($missingRequired -join ', ')"
}

# 2. Local Git Initialization & GitHub Hookup
if ($InitRepo -or (-not (Test-Path (Join-Path $Script:RepoRoot ".git")))) {
    Write-AetherLog "STEP" "INITIALIZING LOCAL GIT REPOSITORY"
    if (-not (Test-Path (Join-Path $Script:RepoRoot ".git"))) {
        git init
        Write-AetherLog "SUCCESS" "Git repository initialized."
    }
    git config core.hooksPath .githooks
    Write-AetherLog "SUCCESS" "Configured core.hooksPath -> .githooks"

    # Attempt GitHub repo creation via gh if present and logged in
    $ghCmd = Get-Command gh -ErrorAction SilentlyContinue
    if ($ghCmd) {
        $ghAuth = & gh auth status 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-AetherLog "INFO" "GitHub CLI authenticated. Ensuring remote repository exists..."
            $repoCheck = & gh repo view 2>&1
            if ($LASTEXITCODE -ne 0) {
                Write-AetherLog "INFO" "Creating private GitHub repository via gh CLI..."
                gh repo create aetherstream --private --source=. --remote=origin --push
                if ($LASTEXITCODE -ne 0) { Write-AetherLog "WARN" "Remote push deferred." }
            }
        } else {
            Write-AetherLog "WARN" "GitHub CLI installed but not authenticated. Run 'gh auth login' to connect remote repository."
        }
    } else {
        Write-AetherLog "WARN" "GitHub CLI (gh) not present on PATH. Remote repo creation deferred."
    }
}

# 3. Status Display
$state = Get-AetherState
if ($Status) {
    Show-AetherStatusDashboard -State $state
    exit 0
}

# 4. Target Execution Determination
$targetSubphase = $null
if ($Subphase) {
    $rec = Get-SubphaseRecord -State $state -SubphaseId $Subphase
    if (-not $rec) {
        throw "Specified Subphase not found in state tracker: $Subphase"
    }
    $targetSubphase = $rec
} elseif ($RunNext -or $Interactive) {
    $rec = Get-NextPendingSubphase -State $state
    if (-not $rec) {
        Write-AetherLog "SUCCESS" "All stages, phases, and subphases are COMPLETED! System is at 100% production readiness."
        Show-AetherStatusDashboard -State $state
        exit 0
    }
    $targetSubphase = $rec
} else {
    Show-AetherStatusDashboard -State $state
    Write-Host "Usage:" -ForegroundColor Cyan
    Write-Host "  .\bootstrap.ps1 -RunNext [-AutoApprove]      # Execute the next pending subphase"
    Write-Host "  .\bootstrap.ps1 -Interactive                # Interactive step-by-step runner"
    Write-Host "  .\bootstrap.ps1 -Subphase 1.1.1             # Run a specific subphase"
    Write-Host "  .\bootstrap.ps1 -Status                     # Show project completion status"
    Write-Host "  .\bootstrap.ps1 -InitRepo                   # Initialize git repository"
    exit 0
}

# 5. Execute Target Subphase
$sub = $targetSubphase.Subphase
$stageNum = $targetSubphase.Stage.number
$stageScript = Join-Path $Script:RepoRoot "scripts/stages/stage-$stageNum.ps1"

if (-not (Test-Path $stageScript)) {
    throw "Stage runner script missing: $stageScript"
}

Write-AetherLog "STEP" "DISPATCHING SUBPHASE $($sub.id) TO STAGE $stageNum RUNNER"
& $stageScript -Subphase $sub.id -AutoApprove:$AutoApprove -ValidateOnly:$ValidateOnly

# 6. Display Updated Progress
$updatedState = Get-AetherState
Show-AetherStatusDashboard -State $updatedState

if ($Interactive) {
    $next = Get-NextPendingSubphase -State $updatedState
    if ($next) {
        Write-Host "Proceed to next subphase ($($next.Subphase.id) - $($next.Subphase.title))? (y/N): " -NoNewline -ForegroundColor Green
        $ans = Read-Host
        if ($ans -eq "y" -or $ans -eq "Y") {
            & $PSCommandPath -Interactive
        }
    }
}
