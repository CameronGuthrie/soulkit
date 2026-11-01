# Package only the GUI and release documentation. Never copy DLLs or target folders.
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('main', 'public')]
    [string]$Channel,
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$CommitSha,
    [string]$OutputDirectory = 'dist/releases'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$outputRoot = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    [IO.Path]::GetFullPath($OutputDirectory)
} else {
    [IO.Path]::GetFullPath((Join-Path $workspaceRoot $OutputDirectory))
}

Push-Location -LiteralPath $workspaceRoot
try {
    $metadataText = & cargo metadata --format-version 1 --no-deps --locked
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = $metadataText | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -EQ 'soulkit').version
    if ($version -notmatch "^\d+\.\d+\.\d+-$Channel\.\d+$") {
        throw "Expected a $Channel push-build version, got '$version'. Stamp the version before building."
    }
    $executable = Join-Path $metadata.target_directory 'release/soulkit.exe'
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Build the Windows GUI first; missing $executable."
    }
    $binaryVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable).ProductVersion
    if ($binaryVersion -ne $version) {
        throw "The executable version '$binaryVersion' does not match Cargo '$version'. Rebuild before packaging."
    }

    $archiveName = "Soulkit-$version-windows-x64.zip"
    $packageRoot = Join-Path $outputRoot "Soulkit-$version-windows-x64"
    if (Test-Path -LiteralPath $packageRoot) {
        throw "Refusing to reuse an existing package folder: $packageRoot. Choose a fresh output directory."
    }
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $packageRoot 'soulkit.exe')
    Copy-Item -LiteralPath (Join-Path $workspaceRoot 'soulkit/RELEASE-SETUP.md') -Destination (Join-Path $packageRoot 'SETUP.md')
    Copy-Item -LiteralPath (Join-Path $workspaceRoot 'LICENSE') -Destination (Join-Path $packageRoot 'LICENSE')
    $buildInfo = [ordered]@{
        version = $version
        channel = $Channel
        commit = $CommitSha
        target = 'x86_64-pc-windows-msvc'
        runtime_dlls_included = $false
    }
    $buildInfo | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $packageRoot 'BUILD-INFO.json') -Encoding utf8

    # An explicit allowlist keeps local lib/, exports and build dependencies out.
    $files = @('soulkit.exe', 'SETUP.md', 'BUILD-INFO.json', 'LICENSE')
    $archive = Join-Path $outputRoot $archiveName
    if (Test-Path -LiteralPath $archive) { throw "Archive already exists: $archive." }
    Compress-Archive -LiteralPath @($files | ForEach-Object { Join-Path $packageRoot $_ }) -DestinationPath $archive
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        $entries = @($zip.Entries | ForEach-Object FullName | Sort-Object)
        if (Compare-Object ($files | Sort-Object) $entries) {
            throw 'Release ZIP contents did not match the four-file allowlist.'
        }
    } finally { $zip.Dispose() }
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    # The publishing job checks this on Linux; filenames must not retain CRLF.
    [IO.File]::WriteAllText((Join-Path $outputRoot 'SHA256SUMS.txt'), "$hash  $archiveName`n", [Text.UTF8Encoding]::new($false))
    Write-Host "Release archive (no DLLs): $archive"
} finally {
    Pop-Location
}
