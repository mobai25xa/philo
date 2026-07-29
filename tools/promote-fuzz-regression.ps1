param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("sse_decoder", "openai_stream", "anthropic_stream", "endpoint_and_headers", "domain_schema_history_tools", "raw_body_and_error", "config_parser")]
    [string]$Target,
    [Parameter(Mandatory = $true)]
    [string]$CrashPath,
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot ".."))
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path -LiteralPath $CrashPath).Path
$repo = (Resolve-Path -LiteralPath $Root).Path
$corpusRoot = Join-Path $repo (Join-Path "fuzz\corpus" $Target)
$resolvedCorpusRoot = (Resolve-Path -LiteralPath $corpusRoot).Path
if (-not $resolvedCorpusRoot.StartsWith($repo, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Corpus path escaped the repository"
}

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant()
$extension = [IO.Path]::GetExtension($source)
if (-not $extension) { $extension = ".bin" }
$destination = Join-Path $resolvedCorpusRoot ("regression-{0}{1}" -f $hash.Substring(0, 16), $extension)
Copy-Item -LiteralPath $source -Destination $destination -ErrorAction Stop
Write-Host "Promoted $source to $destination"
Write-Host "Verify with: cargo test --test fuzz_regression_contract"
