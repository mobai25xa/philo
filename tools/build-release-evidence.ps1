param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$CandidateSha,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:-(?:beta|rc)\.[0-9]+)?$')]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$SourcePackage,

    [Parameter(Mandatory = $true)]
    [string]$CiRunUrl,

    [Parameter(Mandatory = $true)]
    [string]$FuzzRunUrl,

    [Parameter(Mandatory = $true)]
    [string]$PerformanceRunUrl,

    [Parameter(Mandatory = $true)]
    [string]$OpenAiCanaryRunUrl,

    [Parameter(Mandatory = $true)]
    [string]$AnthropicCanaryRunUrl,

    [Parameter(Mandatory = $true)]
    [string]$ApiReviewer,

    [Parameter(Mandatory = $true)]
    [string]$ReleaseReviewer,

    [string]$OutputDirectory = "target/release-gate"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($ApiReviewer -eq $ReleaseReviewer) {
    throw "API and Release reviewers must be distinct"
}

$actualSha = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $actualSha -ne $CandidateSha) {
    throw "Checkout $actualSha does not match candidate $CandidateSha"
}
if (git status --porcelain) {
    throw "Release evidence must be generated from a clean candidate tree"
}

$metadata = cargo metadata --locked --format-version 1 | ConvertFrom-Json -Depth 100
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed"
}
$core = $metadata.packages | Where-Object { $_.name -eq "philo" -and $_.manifest_path -eq (Join-Path $metadata.workspace_root "Cargo.toml") }
if (-not $core -or $core.version -ne $Version) {
    throw "Cargo version does not match release version $Version"
}

$source = Resolve-Path -LiteralPath $SourcePackage
if (-not $source.Path.EndsWith("philo-$Version.crate", [System.StringComparison]::Ordinal)) {
    throw "Source package name does not match philo $Version"
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
$sbomPath = Join-Path $outputRoot "philo-$Version.spdx.json"
$manifestPath = Join-Path $outputRoot "release-manifest.json"
$notesPath = Join-Path $outputRoot "release-notes.md"

$created = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$spdxPackages = @($metadata.packages | Sort-Object name, version | ForEach-Object {
    $license = if ([string]::IsNullOrWhiteSpace($_.license)) { "NOASSERTION" } else { $_.license }
    [ordered]@{
        SPDXID = "SPDXRef-Package-$($_.name -replace '[^A-Za-z0-9.-]', '-')-$($_.version -replace '[^A-Za-z0-9.-]', '-')"
        name = $_.name
        versionInfo = $_.version
        downloadLocation = "NOASSERTION"
        filesAnalyzed = $false
        licenseConcluded = "NOASSERTION"
        licenseDeclared = $license
        copyrightText = "NOASSERTION"
        externalRefs = @(
            [ordered]@{
                referenceCategory = "PACKAGE-MANAGER"
                referenceType = "purl"
                referenceLocator = "pkg:cargo/$($_.name)@$($_.version)"
            }
        )
    }
})

$sbom = [ordered]@{
    spdxVersion = "SPDX-2.3"
    dataLicense = "CC0-1.0"
    SPDXID = "SPDXRef-DOCUMENT"
    name = "philo-$Version"
    documentNamespace = "https://github.com/mobai25xa/philo/releases/philo-v$Version/$CandidateSha"
    creationInfo = [ordered]@{
        created = $created
        creators = @("Tool: philo-release-gate/1")
    }
    documentDescribes = @("SPDXRef-Package-philo-$Version")
    packages = $spdxPackages
}
$sbom | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $sbomPath -Encoding utf8NoBOM

$changelog = Get-Content -Raw -LiteralPath "CHANGELOG.md"
$escapedVersion = [regex]::Escape($Version)
$releaseMatch = [regex]::Match(
    $changelog,
    "(?ms)^## (?:\[$escapedVersion\]|$escapedVersion)\s*\r?\n(?<body>.*?)(?=^## |\z)"
)
if (-not $releaseMatch.Success) {
    throw "CHANGELOG.md has no release section for $Version"
}
$releaseNotes = "# philo $Version`n`n" + $releaseMatch.Groups["body"].Value.Trim() + "`n"
$releaseNotes | Set-Content -LiteralPath $notesPath -Encoding utf8NoBOM

$sourceDigest = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant()
$sbomDigest = (Get-FileHash -Algorithm SHA256 -LiteralPath $sbomPath).Hash.ToLowerInvariant()
$lockDigest = (Get-FileHash -Algorithm SHA256 -LiteralPath "Cargo.lock").Hash.ToLowerInvariant()
$rustc = (rustc --version --verbose) -join "`n"

$manifest = [ordered]@{
    schema = "philo/release-manifest"
    schema_version = 1
    status = "Ready"
    candidate = [ordered]@{
        sha = $CandidateSha
        version = $Version
        tag = "philo-v$Version"
        packages = @("philo")
    }
    source = [ordered]@{
        package = Split-Path -Leaf $source.Path
        sha256 = $sourceDigest
        cargo_lock_sha256 = $lockDigest
        sbom = Split-Path -Leaf $sbomPath
        sbom_sha256 = $sbomDigest
    }
    toolchain = [ordered]@{
        rustc = $rustc
        msrv = $core.rust_version
        features = @("default", "no-default-features", "all-features")
        platforms = @("linux", "windows", "macos")
    }
    evidence = [ordered]@{
        ci = $CiRunUrl
        fuzz = $FuzzRunUrl
        performance = $PerformanceRunUrl
        openai_canary = $OpenAiCanaryRunUrl
        anthropic_canary = $AnthropicCanaryRunUrl
    }
    review = [ordered]@{
        api = $ApiReviewer
        release = $ReleaseReviewer
    }
    provenance = [ordered]@{
        subject_sha256 = @($sourceDigest, $sbomDigest)
        generator = "GitHub artifact attestation in the Release workflow"
    }
    generated_at = $created
}
$manifest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $manifestPath -Encoding utf8NoBOM

Write-Output $manifestPath
