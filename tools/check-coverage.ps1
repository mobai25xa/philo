param(
    [string]$Coverage = "target/coverage/coverage.json",
    [string]$Policy = "support/coverage-policy.json"
)

$ErrorActionPreference = "Stop"
$coverageData = Get-Content -Raw -LiteralPath $Coverage | ConvertFrom-Json -Depth 100
$policyData = Get-Content -Raw -LiteralPath $Policy | ConvertFrom-Json -Depth 20
if ($policyData.schema -ne "philo/coverage-policy") { throw "Unsupported coverage policy schema" }
if (-not $coverageData.data -or $coverageData.data.Count -ne 1) { throw "Expected one LLVM coverage data set" }

$report = $coverageData.data[0]
$overall = [double]$report.totals.lines.percent
$minimumOverall = [Math]::Max(
    [double]$policyData.overall.minimum_line_percent,
    [double]$policyData.overall.baseline_line_percent
)
if ($overall -lt $minimumOverall) {
    throw "Overall line coverage $overall% is below $minimumOverall%"
}

foreach ($group in $policyData.risk_groups) {
    $files = @($report.files | Where-Object {
        $normalized = $_.filename.Replace('\', '/')
        @($group.path_patterns | Where-Object { $normalized.Contains($_) }).Count -gt 0
    })
    if ($files.Count -eq 0) { throw "Coverage group '$($group.name)' matched no files" }
    $lineCount = [double](($files | ForEach-Object { $_.summary.lines.count } | Measure-Object -Sum).Sum)
    $lineCovered = [double](($files | ForEach-Object { $_.summary.lines.covered } | Measure-Object -Sum).Sum)
    $branchCount = [double](($files | ForEach-Object { $_.summary.branches.count } | Measure-Object -Sum).Sum)
    $branchCovered = [double](($files | ForEach-Object { $_.summary.branches.covered } | Measure-Object -Sum).Sum)
    $linePercent = if ($lineCount -eq 0) { 100.0 } else { 100.0 * $lineCovered / $lineCount }
    if ($branchCount -eq 0) {
        throw "Coverage group '$($group.name)' has no branch data; generate coverage with --branch"
    }
    $branchPercent = 100.0 * $branchCovered / $branchCount
    if ($linePercent -lt [double]$group.minimum_line_percent) {
        throw "Coverage group '$($group.name)' line coverage $linePercent% is below $($group.minimum_line_percent)%"
    }
    if ($branchPercent -lt [double]$group.minimum_branch_percent) {
        throw "Coverage group '$($group.name)' branch coverage $branchPercent% is below $($group.minimum_branch_percent)%"
    }
    Write-Host ("{0}: lines={1:N2}% branches={2:N2}%" -f $group.name, $linePercent, $branchPercent)
}
Write-Host ("overall: lines={0:N2}%" -f $overall)
