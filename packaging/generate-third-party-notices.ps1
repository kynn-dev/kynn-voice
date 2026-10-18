param([string]$OutputPath = 'THIRD_PARTY_LICENSES.txt')
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location -LiteralPath $root
try {
    $raw = & cargo metadata --format-version 1 --locked --offline --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Locked dependency metadata failed.' }
    $metadata = ($raw -join "`n") | ConvertFrom-Json
    $resolved = @{}
    foreach ($node in $metadata.resolve.nodes) { $resolved[$node.id] = $true }
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add('Kynn Voice - third-party license texts')
    $lines.Add('Generated from Cargo.lock and locally verified Cargo registry sources.')
    $lines.Add('Includes the resolved default-feature Windows graph, including build tools.')
    $lines.Add('MIT licensing of Kynn Voice does not replace these component licenses.')
    $missing = [System.Collections.Generic.List[string]]::new()
    foreach ($package in ($metadata.packages | Where-Object { $_.id -ne $metadata.resolve.root -and $resolved.ContainsKey($_.id) } | Sort-Object name,version)) {
        $directory = Split-Path -Parent $package.manifest_path
        $files = @(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([._-].*)?$' })
        if ($package.license_file) {
            $custom = Join-Path $directory $package.license_file
            if (Test-Path -LiteralPath $custom -PathType Leaf) { $files += Get-Item -LiteralPath $custom }
        }
        $supplement = Join-Path $root "licenses/crates/$($package.name)-$($package.version).txt"
        if ($files.Count -eq 0 -and (Test-Path -LiteralPath $supplement -PathType Leaf)) {
            $files += Get-Item -LiteralPath $supplement
        }
        $files = @($files | Sort-Object FullName -Unique)
        $lines.Add('')
        $lines.Add(('=' * 72))
        $lines.Add("$($package.name) $($package.version)")
        $lines.Add("Declared license: $($package.license)")
        if ($package.repository) { $lines.Add("Source: $($package.repository)") }
        if ($files.Count -eq 0) { $missing.Add("$($package.name) $($package.version)"); continue }
        foreach ($file in $files) {
            $lines.Add('')
            $lines.Add("--- $($file.Name) ---")
            $lines.Add([System.IO.File]::ReadAllText($file.FullName))
        }
    }
    if ($missing.Count) { throw ('License texts missing; inspect sources before distribution: ' + ($missing -join ', ')) }
    $lines | Set-Content -LiteralPath $OutputPath -Encoding utf8
    Write-Output "LICENSE_TEXTS_PASS $($resolved.Count - 1) dependency nodes; $OutputPath"
} finally {
    Pop-Location
}
