param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot ".."))
)

$ErrorActionPreference = "Stop"
$rootPath = (Resolve-Path -LiteralPath $Root).Path
$targetPath = Join-Path $rootPath "target"
$markdownFiles = @(
    Get-ChildItem -LiteralPath $rootPath -Recurse -File -Filter "*.md" |
        Where-Object { -not $_.FullName.StartsWith($targetPath) } |
        Sort-Object FullName
)
$errors = [System.Collections.Generic.List[string]]::new()
$linkPattern = [regex]"!?\[[^\]]*\]\((?<target><[^>]+>|[^)]+)\)"

foreach ($file in $markdownFiles) {
    $lines = @(Get-Content -LiteralPath $file.FullName)
    $inFence = $false
    $fenceMarker = ""
    for ($index = 0; $index -lt $lines.Count; $index++) {
        $line = $lines[$index]
        if ($line -match '^\s*(?<marker>`{3}|~{3})') {
            if (-not $inFence) {
                $inFence = $true
                $fenceMarker = $Matches.marker
            }
            elseif ($Matches.marker -eq $fenceMarker) {
                $inFence = $false
                $fenceMarker = ""
            }
            continue
        }
        if ($inFence) {
            continue
        }

        foreach ($match in $linkPattern.Matches($line)) {
            $target = $match.Groups["target"].Value.Trim()
            if ($target.StartsWith("<") -and $target.EndsWith(">")) {
                $target = $target.Substring(1, $target.Length - 2)
            }
            if ($target -match '^(?i:https?://|mailto:)' -or
                $target -match '^[a-zA-Z][a-zA-Z0-9+.-]*:') {
                continue
            }
            $pathPart = (($target -split '#', 2)[0] -split '\?', 2)[0]
            if ([string]::IsNullOrWhiteSpace($pathPart)) {
                continue
            }
            try {
                $decoded = [Uri]::UnescapeDataString($pathPart)
                $resolved = [IO.Path]::GetFullPath((Join-Path $file.DirectoryName $decoded))
            }
            catch {
                $errors.Add("$($file.FullName):$($index + 1): invalid path: $target")
                continue
            }
            if (-not $resolved.StartsWith($rootPath, [StringComparison]::OrdinalIgnoreCase)) {
                $errors.Add("$($file.FullName):$($index + 1): link leaves repository: $target")
            }
            elseif (-not (Test-Path -LiteralPath $resolved)) {
                $errors.Add("$($file.FullName):$($index + 1): target does not exist: $target")
            }
        }
    }
    if ($inFence) {
        $errors.Add("$($file.FullName): unclosed fenced code block")
    }
}

if ($errors.Count -gt 0) {
    $errors | Sort-Object | Write-Output
    throw "Markdown validation failed with $($errors.Count) issue(s)"
}

Write-Output "Markdown validation passed: $($markdownFiles.Count) file(s), 0 broken local link(s)"
