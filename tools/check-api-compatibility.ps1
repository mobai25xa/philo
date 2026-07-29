param(
    [string]$BaselineRev = "",
    [string]$Package = "philo",
    [ValidateSet("default", "no-default-features", "all-features")]
    [string[]]$FeatureSets = @("default", "no-default-features", "all-features"),
    [string]$OutputDirectory = "target/compatibility/api",
    [string]$ApprovalFile = "",
    [switch]$AllowBootstrap
)

$ErrorActionPreference = "Stop"

function Write-JsonReport {
    param([hashtable]$Report, [string]$Path)

    $directory = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $Report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

if ([string]::IsNullOrWhiteSpace($BaselineRev)) {
    $BaselineRev = git tag --list "philo-v[0-9]*" --sort=-version:refname |
        Select-Object -First 1
}

$subject = (git rev-parse HEAD).Trim()
$reportPath = Join-Path $OutputDirectory "summary.json"

if ([string]::IsNullOrWhiteSpace($BaselineRev)) {
    $report = @{
        schema_version = 1
        package = $Package
        subject = $subject
        baseline = $null
        status = "bootstrap-pending"
        feature_sets = $FeatureSets
        reason = "No philo-v* stable tag exists. API and Release sign-off plus baseline review are required before the first baseline is created."
    }
    Write-JsonReport -Report $report -Path $reportPath
    if ($AllowBootstrap) {
        Write-Host "API compatibility bootstrap pending; report: $reportPath"
        exit 0
    }
    throw "API compatibility baseline is required but no philo-v* stable tag exists"
}

& cargo semver-checks --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "cargo-semver-checks is required; install it with: cargo install cargo-semver-checks --locked"
}

$results = @()
$failed = $false
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

foreach ($featureSet in $FeatureSets) {
    $arguments = @(
        "semver-checks", "check-release",
        "--package", $Package,
        "--baseline-rev", $BaselineRev
    )
    switch ($featureSet) {
        "no-default-features" { $arguments += "--no-default-features" }
        "all-features" { $arguments += "--all-features" }
    }

    $logPath = Join-Path $OutputDirectory "$featureSet.log"
    & cargo @arguments 2>&1 | Tee-Object -FilePath $logPath
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        $failed = $true
    }
    $results += @{
        feature_set = $featureSet
        exit_code = $exitCode
        log = $logPath.Replace('\', '/')
    }
}

$status = if ($failed) { "breaking-change-detected" } else { "compatible" }
$approval = $null
if ($failed -and [string]::IsNullOrWhiteSpace($ApprovalFile)) {
    $matchingApprovals = Get-ChildItem "compatibility/approvals" -Filter "*.toml" -File |
        Where-Object {
            $candidate = [IO.File]::ReadAllText($_.FullName)
            $candidate.Contains('status = "approved"') -and
                $candidate.Contains("baseline_ref = `"$BaselineRev`"")
        }
    if (@($matchingApprovals).Count -eq 1) {
        $ApprovalFile = @($matchingApprovals)[0].FullName
    } elseif (@($matchingApprovals).Count -gt 1) {
        throw "multiple approved breaking records match baseline $BaselineRev"
    }
}
if ($failed -and -not [string]::IsNullOrWhiteSpace($ApprovalFile)) {
    $resolvedApproval = (Resolve-Path $ApprovalFile).Path
    $approvalRoot = (Resolve-Path "compatibility/approvals").Path
    if (-not $resolvedApproval.StartsWith($approvalRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "breaking approval must live under compatibility/approvals"
    }
    $approvalText = [IO.File]::ReadAllText($resolvedApproval)
    foreach ($required in @(
        'kind = "api"',
        'status = "approved"',
        "baseline_ref = `"$BaselineRev`""
    )) {
        if (-not $approvalText.Contains($required)) {
            throw "breaking approval lacks required field: $required"
        }
    }
    $adr = [regex]::Match($approvalText, '(?m)^adr = "([^"]+)"$').Groups[1].Value
    $migration = [regex]::Match($approvalText, '(?m)^migration = "([^"]+)"$').Groups[1].Value
    $apiReviewer = [regex]::Match($approvalText, '(?m)^api_reviewer = "([^"]+)"$').Groups[1].Value
    $releaseReviewer = [regex]::Match($approvalText, '(?m)^release_reviewer = "([^"]+)"$').Groups[1].Value
    $targetVersion = [regex]::Match($approvalText, '(?m)^target_version = "([1-9][0-9]*\.0\.0)"$').Groups[1].Value
    if (-not (Test-Path -LiteralPath $adr -PathType Leaf)) { throw "approval ADR does not exist: $adr" }
    if (-not (Test-Path -LiteralPath $migration -PathType Leaf)) { throw "approval migration does not exist: $migration" }
    if ([string]::IsNullOrWhiteSpace($apiReviewer) -or $apiReviewer -eq $releaseReviewer) {
        throw "approval requires distinct API and Release reviewers"
    }
    if ([string]::IsNullOrWhiteSpace($targetVersion)) {
        throw "approval target_version must be a new nonzero major release"
    }
    $approval = $ApprovalFile.Replace('\', '/')
    $status = "approved-breaking-change"
    $failed = $false
}
Write-JsonReport -Report @{
    schema_version = 1
    package = $Package
    subject = $subject
    baseline = $BaselineRev
    status = $status
    approval = $approval
    results = $results
} -Path $reportPath

if ($failed) {
    throw "API compatibility check failed; see $reportPath and per-feature logs"
}

Write-Host "API compatibility check passed; report: $reportPath"
