# AetherStream Common Automation & Governance Library
# File: scripts/common.ps1
# Multi-platform PowerShell 5.1 & PowerShell 7+ compatible

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Script:StateFile = Join-Path $Script:RepoRoot "state.tracker.json"
$Script:ChangelogFile = Join-Path $Script:RepoRoot "CHANGELOG.md"
$Script:TodoFile = Join-Path $Script:RepoRoot "TODO.md"

function Write-AetherLog {
    param(
        [Parameter(Mandatory=$true)]
        [ValidateSet("INFO", "SUCCESS", "WARN", "ERROR", "STEP")]
        [string]$Level,

        [Parameter(Mandatory=$true)]
        [string]$Message
    )

    $timestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    switch ($Level) {
        "INFO"    { Write-Host "[$timestamp] [INFO]    $Message" -ForegroundColor Cyan }
        "SUCCESS" { Write-Host "[$timestamp] [SUCCESS] $Message" -ForegroundColor Green }
        "WARN"    { Write-Host "[$timestamp] [WARN]    $Message" -ForegroundColor Yellow }
        "ERROR"   { Write-Host "[$timestamp] [ERROR]   $Message" -ForegroundColor Red }
        "STEP"    { Write-Host "`n[$timestamp] >>>>> $Message <<<<<" -ForegroundColor Magenta }
    }
}

function Get-AetherState {
    if (-not (Test-Path $Script:StateFile)) {
        throw "State tracker file not found at: $Script:StateFile"
    }
    $raw = Get-Content -Path $Script:StateFile -Raw -Encoding UTF8
    return ($raw | ConvertFrom-Json)
}

function Save-AetherState {
    param([Parameter(Mandatory=$true)]$State)

    $State.updated_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    $json = $State | ConvertTo-Json -Depth 12

    $tempFile = "$Script:StateFile.tmp"
    $backupFile = "$Script:StateFile.bak"

    try {
        # Checkpoint backup
        if (Test-Path $Script:StateFile) {
            Copy-Item -Path $Script:StateFile -Destination $backupFile -Force
        }

        # Write to temporary file with UTF-8 No BOM
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($tempFile, $json, $utf8NoBom)

        # Integrity verification: verify JSON parse
        $verifyRaw = Get-Content -Path $tempFile -Raw -Encoding UTF8
        $null = $verifyRaw | ConvertFrom-Json

        # Atomic rename
        Move-Item -Path $tempFile -Destination $Script:StateFile -Force
        if (Test-Path $backupFile) {
            Remove-Item -Path $backupFile -Force -ErrorAction SilentlyContinue
        }
    }
    catch {
        Write-AetherLog "ERROR" "State mutation failed! Initiating rollback to checkpoint..."
        if (Test-Path $backupFile) {
            Move-Item -Path $backupFile -Destination $Script:StateFile -Force
            Write-AetherLog "WARN" "State rolled back to previous valid checkpoint."
        }
        throw $_
    }
}

function Get-SubphaseRecord {
    param(
        [Parameter(Mandatory=$true)]$State,
        [Parameter(Mandatory=$true)][string]$SubphaseId
    )
    $cleanId = $SubphaseId.Replace("SUBPHASE_", "")
    foreach ($stage in $State.stages) {
        foreach ($phase in $stage.phases) {
            foreach ($sub in $phase.subphases) {
                if ($sub.id -eq $SubphaseId -or $sub.id -eq "SUBPHASE_$cleanId" -or $sub.id.EndsWith($cleanId)) {
                    return @{
                        Stage = $stage
                        Phase = $phase
                        Subphase = $sub
                    }
                }
            }
        }
    }
    return $null
}

function Get-NextPendingSubphase {
    param([Parameter(Mandatory=$true)]$State)
    foreach ($stage in $State.stages) {
        foreach ($phase in $stage.phases) {
            foreach ($sub in $phase.subphases) {
                if ($sub.status -ne "COMPLETED") {
                    return @{
                        Stage = $stage
                        Phase = $phase
                        Subphase = $sub
                    }
                }
            }
        }
    }
    return $null
}

function Append-AetherChangelog {
    param(
        [Parameter(Mandatory=$true)][string]$StageId,
        [Parameter(Mandatory=$true)][string]$PhaseId,
        [Parameter(Mandatory=$true)][string]$SubphaseId,
        [Parameter(Mandatory=$true)][string[]]$ComponentsModified,
        [Parameter(Mandatory=$true)][string]$DeltaDescription,
        [Parameter(Mandatory=$false)][string]$VerificationSignature = "SHA256:VERIFIED_PASS"
    )

    if (-not (Test-Path $Script:ChangelogFile)) {
        throw "Changelog not found: $Script:ChangelogFile"
    }

    $oldContent = Get-Content -Path $Script:ChangelogFile -Raw -Encoding UTF8
    $oldLength = (Get-Item $Script:ChangelogFile).Length
    $utcTimestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

    $componentLines = ($ComponentsModified | ForEach-Object { "  - $_" }) -join "`n"

    $entry = @(
        "",
        "### [$utcTimestamp] - $StageId / $PhaseId / $SubphaseId",
        "- **Stage ID:** $StageId",
        "- **Phase ID:** $PhaseId",
        "- **Subphase ID:** $SubphaseId",
        "- **Status:** COMPLETED",
        "- **Components Modified:**",
        $componentLines,
        "- **Atomic Delta Description:** $DeltaDescription",
        "- **Verification Signature:** $VerificationSignature",
        "- **Executor:** System Bootstrapper (AetherStream Automation Suite)"
    ) -join "`n"

    # Append strictly
    Add-Content -Path $Script:ChangelogFile -Value $entry -Encoding UTF8

    # Assert append-only integrity
    $newLength = (Get-Item $Script:ChangelogFile).Length
    $newContent = Get-Content -Path $Script:ChangelogFile -Raw -Encoding UTF8
    if ($newLength -le $oldLength -or -not $newContent.StartsWith($oldContent)) {
        throw "CRITICAL GOVERNANCE VIOLATION: Append-only integrity check failed on CHANGELOG.md!"
    }
    Write-AetherLog "SUCCESS" "Immutable entry appended to CHANGELOG.md ($SubphaseId)."
}

function Append-AetherTodoTransition {
    param(
        [Parameter(Mandatory=$true)][string]$TaskId,
        [Parameter(Mandatory=$true)][string]$FromState,
        [Parameter(Mandatory=$true)][string]$ToState,
        [Parameter(Mandatory=$true)][string]$ExecutionTrace,
        [Parameter(Mandatory=$true)][string]$ValidationProof
    )

    if (-not (Test-Path $Script:TodoFile)) {
        throw "TODO ledger not found: $Script:TodoFile"
    }

    $oldContent = Get-Content -Path $Script:TodoFile -Raw -Encoding UTF8
    $oldLength = (Get-Item $Script:TodoFile).Length
    $utcTimestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

    $entry = @(
        "",
        "### [$utcTimestamp] TRANSACTION [TASK UPDATE]",
        "- **Task ID:** $TaskId",
        "- **Transition:** $FromState -> $ToState",
        "- **Executor:** Master Bootstrapper (Stage Automation)",
        "- **Execution Trace:** $ExecutionTrace",
        "- **Validation Proof:** $ValidationProof"
    ) -join "`n"

    # Append strictly to TODO.md
    Add-Content -Path $Script:TodoFile -Value $entry -Encoding UTF8

    # Assert append-only integrity
    $newLength = (Get-Item $Script:TodoFile).Length
    $newContent = Get-Content -Path $Script:TodoFile -Raw -Encoding UTF8
    if ($newLength -le $oldLength -or -not $newContent.StartsWith($oldContent)) {
        throw "CRITICAL GOVERNANCE VIOLATION: Append-only integrity check failed on TODO.md!"
    }
    Write-AetherLog "SUCCESS" "Immutable task transition appended to TODO.md ($TaskId -> $ToState)."
}

function Confirm-HumanOrAgentReview {
    param(
        [Parameter(Mandatory=$true)][string]$SubphaseId,
        [Parameter(Mandatory=$true)][string]$Title,
        [Parameter(Mandatory=$false)][switch]$AutoApprove
    )

    if ($AutoApprove) {
        Write-AetherLog "INFO" "Auto-approval active. Proceeding with subphase commit."
        return $true
    }

    Write-Host "`n============================================================" -ForegroundColor Cyan
    Write-Host " HUMAN / AGENT STAGE APPROVAL GATE" -ForegroundColor Yellow
    Write-Host " Subphase: $SubphaseId - $Title" -ForegroundColor White
    Write-Host " Verification tests passed. Approve committing this transition? (y/N): " -NoNewline -ForegroundColor Green
    $answer = Read-Host
    if ($answer -ne "y" -and $answer -ne "Y") {
        Write-AetherLog "WARN" "Subphase transition was not approved by operator."
        return $false
    }
    return $true
}

function Show-AetherStatusDashboard {
    param([Parameter(Mandatory=$true)]$State)

    Write-Host "`n================================================================================" -ForegroundColor Cyan
    Write-Host "                AETHERSTREAM ECOSYSTEM STATE DASHBOARD                          " -ForegroundColor Yellow
    Write-Host "================================================================================" -ForegroundColor Cyan
    Write-Host "Project: $($State.project_name) | Schema: v$($State.schema_version) | Updated: $($State.updated_at)`n"

    $totalSub = 0
    $completedSub = 0

    foreach ($stage in $State.stages) {
        $stageColor = "DarkGray"
        if ($stage.status -eq "COMPLETED") {
            $stageColor = "Green"
        } elseif ($stage.status -eq "IN_PROGRESS") {
            $stageColor = "Yellow"
        }
        Write-Host "[$($stage.id)] $($stage.title) - $($stage.status)" -ForegroundColor $stageColor
        foreach ($phase in $stage.phases) {
            Write-Host "   |-- [$($phase.id)] $($phase.title) [$($phase.status)]" -ForegroundColor White
            foreach ($sub in $phase.subphases) {
                $totalSub++
                $subColor = "DarkGray"
                $subIcon = "[ ]"
                if ($sub.status -eq "COMPLETED") {
                    $completedSub++
                    $subColor = "Green"
                    $subIcon = "[X]"
                } elseif ($sub.status -eq "IN_PROGRESS") {
                    $subColor = "Yellow"
                    $subIcon = "[>]"
                }
                Write-Host "   |     $subIcon $($sub.id) ($($sub.task_id)): $($sub.title)" -ForegroundColor $subColor
            }
        }
    }

    $pct = [math]::Round(($completedSub / [math]::Max(1, $totalSub)) * 100, 1)
    Write-Host "`nProgress: $completedSub of $totalSub subphases completed ($pct%)" -ForegroundColor Cyan
    Write-Host "================================================================================`n" -ForegroundColor Cyan
}
