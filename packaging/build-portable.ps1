param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [string]$OutputDirectory = 'target/public-release'
)
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$app = [System.IO.Path]::GetFullPath($AppPath)
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
if (-not (Test-Path -LiteralPath $app -PathType Leaf)) { throw 'Compiled application not found.' }
$manifest = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
$match = [regex]::Match($manifest, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"')
if (-not $match.Success) { throw 'Package version was not found.' }
$version = $match.Groups[1].Value
$fileVersion = (Get-Item -LiteralPath $app).VersionInfo.FileVersion
if ($fileVersion -ne $version -and $fileVersion -ne "$version.0") { throw "Executable version $fileVersion does not match $version" }
New-Item -ItemType Directory -Path $output -Force | Out-Null
$stage = Join-Path $output 'portable'
New-Item -ItemType Directory -Path $stage -Force | Out-Null
$exeName = "KynnVoice-$version-windows-x64.exe"
$zipName = "KynnVoice-$version-windows-x64-portable.zip"
Copy-Item -LiteralPath $app -Destination (Join-Path $output $exeName) -Force
Copy-Item -LiteralPath $app -Destination (Join-Path $stage 'kynn-voice.exe') -Force
$entries = @('kynn-voice.exe')
foreach ($file in @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt')) {
    Copy-Item -LiteralPath (Join-Path $root $file) -Destination (Join-Path $stage $file) -Force
    Copy-Item -LiteralPath (Join-Path $root $file) -Destination (Join-Path $output $file) -Force
    $entries += $file
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'PORTABLE-README.txt') -Destination (Join-Path $stage 'LEIA-ME.txt') -Force
$entries += 'LEIA-ME.txt'
$paths = @($entries | ForEach-Object { Join-Path $stage $_ })
Compress-Archive -LiteralPath $paths -DestinationPath (Join-Path $output $zipName) -CompressionLevel Optimal -Force
# Explicit files only: a stale staging file can never enter the public archive.
$checks = foreach ($name in @($exeName, $zipName, 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt')) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $output $name)).Hash.ToLowerInvariant()
    "$hash  $name"
}
$checks | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
Write-Output $checks
