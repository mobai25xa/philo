param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")),
    [string]$ExpectedCandidate,
    [switch]$RequireCleanCandidate,
    [switch]$IncludeBenchmark,
    [int]$QuickSoakIterations = 1000
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path -LiteralPath $Root).Path
$manifest = Join-Path $repo "Cargo.toml"

function Invoke-GateCommand {
    param([scriptblock]$Command, [string]$Name)
    Write-Host "[phase4] $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "Phase 4 gate command failed: $Name"
    }
}

$candidate = (& git -C $repo rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $candidate -notmatch '^[0-9a-f]{40}$') {
    throw "Unable to resolve a full candidate SHA"
}
if ($ExpectedCandidate -and $candidate -ne $ExpectedCandidate) {
    throw "Checked out SHA $candidate does not match expected candidate $ExpectedCandidate"
}
if ($RequireCleanCandidate) {
    $dirty = & git -C $repo status --porcelain
    if ($LASTEXITCODE -ne 0 -or $dirty) {
        throw "Exact-SHA gate requires a clean candidate worktree"
    }
}

Write-Host "[phase4] candidate=$candidate"
Invoke-GateCommand { cargo fmt --manifest-path $manifest --all -- --check } "rustfmt"
Invoke-GateCommand { cargo clippy --manifest-path $manifest --all-targets --all-features -- -D warnings } "clippy"
Invoke-GateCommand { cargo test --manifest-path $manifest --all-targets --all-features } "all targets and features"
Invoke-GateCommand { cargo doc --manifest-path $manifest --all-features --no-deps } "rustdoc"
Invoke-GateCommand { cargo check --manifest-path $manifest --examples --all-features } "examples"
Invoke-GateCommand { cargo check --manifest-path $manifest --no-default-features } "no default features"
Invoke-GateCommand { pwsh -File (Join-Path $repo "tools\check-markdown-links.ps1") -Root $repo } "tracked Markdown"
Invoke-GateCommand { cargo test --manifest-path $manifest --all-features --test phase4_reliability_contract --test phase4_fault_matrix } "fault and security contracts"

if ($IncludeBenchmark) {
    Invoke-GateCommand { cargo bench --manifest-path $manifest --bench phase4_sdk -- --smoke } "benchmark smoke"
    Invoke-GateCommand { cargo bench --manifest-path $manifest --bench phase4_client_soak -- quick $QuickSoakIterations } "quick client soak"
}

Write-Host "[phase4] local gate passed for candidate=$candidate"
