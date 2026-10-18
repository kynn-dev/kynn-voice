param(
    [string]$DriverArchivePath,
    [string]$AppPath,
    [switch]$SkipCargoBuild,
    [switch]$VerifyOnly
)

$ErrorActionPreference = 'Stop'
$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cargoToml = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'Could not determine the Kynn Voice version from Cargo.toml.'
}
$version = $versionMatch.Groups[1].Value

$expectedDriverZipSha256 = 'b950e39f01af1d04ea623c8f6d8eb9b6ea5c477c637295fabf20631c85116bfb'
$officialDriverUrl = 'https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip'
if ($AppPath) {
    if (-not $SkipCargoBuild) { throw '-AppPath requires -SkipCargoBuild so the selected binary is not overwritten.' }
    $appPath = [System.IO.Path]::GetFullPath($AppPath)
} else {
    $appPath = Join-Path $projectRoot 'target\release\kynn-voice.exe'
}
$releaseDirectory = Join-Path $projectRoot 'target\release'
$outputPath = Join-Path $releaseDirectory "KynnVoice-Setup-$version-x64.exe"
$stagePath = Join-Path $releaseDirectory 'kynn-voice-setup-staging'
$auditPath = Join-Path $env:TEMP ('KynnVoice-Setup-Audit-' + [guid]::NewGuid().ToString('N'))
$csharpCompiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Test-AuthenticodePublisher {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$SubjectPattern,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch $SubjectPattern) {
        throw "$Description signature is not valid for the expected publisher. Status=$($signature.Status); Subject=$($signature.SignerCertificate.Subject)"
    }
    return $signature.SignerCertificate.Subject
}

if (-not $DriverArchivePath) {
    $DriverArchivePath = Join-Path $env:TEMP 'VBCABLE_Driver_Pack45.zip'
    if (-not (Test-Path -LiteralPath $DriverArchivePath -PathType Leaf)) {
        Invoke-WebRequest -Uri $officialDriverUrl -OutFile $DriverArchivePath
    }
}
$DriverArchivePath = [System.IO.Path]::GetFullPath($DriverArchivePath)
if (-not (Test-Path -LiteralPath $DriverArchivePath -PathType Leaf)) {
    throw "VB-CABLE archive not found: $DriverArchivePath"
}

$actualDriverZipSha256 = Get-Sha256 -Path $DriverArchivePath
if ($actualDriverZipSha256 -ne $expectedDriverZipSha256) {
    throw "VB-CABLE archive SHA-256 mismatch. Expected $expectedDriverZipSha256; got $actualDriverZipSha256."
}

if (-not $SkipCargoBuild) {
    $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
    if (-not $cargo) {
        $candidate = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $cargo = $candidate
        }
    }
    if (-not $cargo) {
        throw 'cargo.exe was not found. Install the official Rust stable MSVC x64 toolchain or use -SkipCargoBuild with an already built release executable.'
    }

    Push-Location $projectRoot
    try {
        & $cargo build --release --locked --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
    throw "Release executable not found: $appPath"
}
if (-not (Test-Path -LiteralPath $csharpCompiler -PathType Leaf)) {
    throw "Microsoft .NET Framework C# compiler was not found: $csharpCompiler"
}
$appSha256 = Get-Sha256 -Path $appPath
$csharpCompilerPublisher = Test-AuthenticodePublisher -Path $csharpCompiler -SubjectPattern 'Microsoft Windows' -Description '.NET Framework C# compiler'

Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Directory]::CreateDirectory($auditPath) | Out-Null
try {
    [System.IO.Compression.ZipFile]::ExtractToDirectory($DriverArchivePath, $auditPath)
    $driverSetupPath = Join-Path $auditPath 'VBCABLE_Setup_x64.exe'
    $driverCatalogPath = Join-Path $auditPath 'vbaudio_cable64_win10.cat'
    $driverInfPath = Join-Path $auditPath 'vbMmeCable64_win10.inf'
    $driverSysPath = Join-Path $auditPath 'vbaudio_cable64_win10.sys'

    $setupPublisher = Test-AuthenticodePublisher -Path $driverSetupPath -SubjectPattern 'BUREL VINCENT' -Description 'VB-CABLE x64 setup'
    $catalogPublisher = Test-AuthenticodePublisher -Path $driverCatalogPath -SubjectPattern 'Microsoft Windows Hardware Compatibility Publisher' -Description 'VB-CABLE x64 catalog'
    $driverSetupSha256 = Get-Sha256 -Path $driverSetupPath
    $driverCatalogSha256 = Get-Sha256 -Path $driverCatalogPath

    $signtoolCandidates = @()
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (Test-Path -LiteralPath $kitsRoot -PathType Container) {
        $signtoolCandidates = @(Get-ChildItem -LiteralPath $kitsRoot -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.Directory.Name -eq 'x64' } | Sort-Object FullName -Descending)
    }
    if ($signtoolCandidates.Count -eq 0) {
        throw 'Windows SDK SignTool was not found; cannot verify the VB-CABLE catalog membership for its INF and SYS files.'
    }
    $signtool = $signtoolCandidates[0].FullName
    $signtoolPublisher = Test-AuthenticodePublisher -Path $signtool -SubjectPattern 'Microsoft Corporation' -Description 'Windows SDK SignTool'

    foreach ($driverFile in @($driverInfPath, $driverSysPath)) {
        & $signtool verify /kp /v /c $driverCatalogPath $driverFile | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "VB-CABLE catalog verification failed for $([System.IO.Path]::GetFileName($driverFile))."
        }
    }

    if ($VerifyOnly) {
        [PSCustomObject]@{
            Status = 'PASS'
            Version = $version
            AppSha256 = $appSha256
            DriverPackage = 'VB-CABLE Driver Pack 45'
            DriverArchiveSha256 = $actualDriverZipSha256
            DriverSetupSha256 = $driverSetupSha256
            DriverCatalogSha256 = $driverCatalogSha256
            DriverSetupSigner = $setupPublisher
            DriverCatalogSigner = $catalogPublisher
            SignToolSigner = $signtoolPublisher
            CSharpCompilerSigner = $csharpCompilerPublisher
            OutputPath = $outputPath
        } | ConvertTo-Json -Depth 3
        exit 0
    }
}
finally {
    $resolvedAudit = [System.IO.Path]::GetFullPath($auditPath).TrimEnd('\')
    $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
    if ($resolvedAudit.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolvedAudit).StartsWith('KynnVoice-Setup-Audit-')) {
        Remove-Item -LiteralPath $resolvedAudit -Recurse -Force
    }
}

$resolvedStage = [System.IO.Path]::GetFullPath($stagePath).TrimEnd('\')
$resolvedRelease = [System.IO.Path]::GetFullPath($releaseDirectory).TrimEnd('\') + '\'
if (-not $resolvedStage.StartsWith($resolvedRelease, [System.StringComparison]::OrdinalIgnoreCase) -or $resolvedStage -eq $resolvedRelease.TrimEnd('\')) {
    throw "Refusing to write outside the release staging directory: $resolvedStage"
}
if (Test-Path -LiteralPath $resolvedStage) {
    Remove-Item -LiteralPath $resolvedStage -Recurse -Force
}
[System.IO.Directory]::CreateDirectory($resolvedStage) | Out-Null

$stageAppPath = Join-Path $resolvedStage 'kynn-voice.exe'
$stageDriverZipPath = Join-Path $resolvedStage 'VBCABLE_Driver_Pack45.zip'
$stageLicensePath = Join-Path $resolvedStage 'KynnVoice-LICENSE.txt'
$stageNoticePath = Join-Path $resolvedStage 'VB-CABLE-NOTICE.txt'
Copy-Item -LiteralPath $appPath -Destination $stageAppPath
Copy-Item -LiteralPath $DriverArchivePath -Destination $stageDriverZipPath
Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $stageLicensePath
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'VB-CABLE-NOTICE.txt') -Destination $stageNoticePath

$expectedSetupSha256 = '734c35dfa6d98f48782a451633ceb471166ec70d60482fd89a1123d0ee3c4f41'
$expectedCatalogSha256 = '0a921ebadbe39cf3fa7c14ff37d1ca22565a9651244bd3ac177dc810bc99072e'
$sourceTemplate = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'KynnVoiceSetup.cs') -Raw
$source = $sourceTemplate.Replace('__APP_SHA256__', $appSha256).Replace('__DRIVER_ZIP_SHA256__', $expectedDriverZipSha256).Replace('__DRIVER_SETUP_SHA256__', $expectedSetupSha256).Replace('__DRIVER_CATALOG_SHA256__', $expectedCatalogSha256)
$sourcePath = Join-Path $resolvedStage 'KynnVoiceSetup.generated.cs'
[System.IO.File]::WriteAllText($sourcePath, $source, [System.Text.UTF8Encoding]::new($false))

$manifestPath = Join-Path $PSScriptRoot 'setup.manifest'
$iconPath = Join-Path $projectRoot 'img\kynn-voice.ico'
$compilerArguments = @(
    '/nologo',
    '/target:winexe',
    '/platform:x64',
    "/out:$outputPath",
    "/win32icon:$iconPath",
    "/win32manifest:$manifestPath",
    '/reference:System.Windows.Forms.dll',
    '/reference:System.Drawing.dll',
    '/reference:System.IO.Compression.dll',
    '/reference:System.IO.Compression.FileSystem.dll',
    "/resource:$stageAppPath,KynnVoiceApp",
    "/resource:$stageDriverZipPath,VBCableZip",
    "/resource:$stageLicensePath,KynnVoiceLicense",
    "/resource:$stageNoticePath,VBCableNotice",
    $sourcePath
)
& $csharpCompiler $compilerArguments
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $outputPath -PathType Leaf)) {
    throw "The Kynn Voice setup compilation failed. Exit code: $LASTEXITCODE"
}

$payloadCheck = Start-Process -FilePath $outputPath -ArgumentList '--verify-only' -WindowStyle Hidden -Wait -PassThru
if ($payloadCheck.ExitCode -ne 0) {
    throw "The compiled setup did not pass its embedded payload checks. Exit code: $($payloadCheck.ExitCode)"
}

$outputSha256 = Get-Sha256 -Path $outputPath
$hashFile = "$outputPath.sha256"
[System.IO.File]::WriteAllText($hashFile, "$outputSha256  $(Split-Path -Leaf $outputPath)`r`n", [System.Text.Encoding]::ASCII)

[PSCustomObject]@{
    Status = 'BUILT'
    OutputPath = $outputPath
    OutputSha256 = $outputSha256
    SizeBytes = (Get-Item -LiteralPath $outputPath).Length
    AuthenticodeStatus = (Get-AuthenticodeSignature -LiteralPath $outputPath).Status.ToString()
    AppSha256 = $appSha256
    DriverArchiveSha256 = $actualDriverZipSha256
    DriverSetupSigner = $setupPublisher
    DriverCatalogSigner = $catalogPublisher
    CSharpCompilerSigner = $csharpCompilerPublisher
} | ConvertTo-Json -Depth 3
