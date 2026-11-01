# Package the Soulkit GUI and its required runtime DLLs into a portable folder.
[CmdletBinding()]
param(
    [string]$OutputDirectory = 'dist/Soulkit'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$packageRoot = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    [IO.Path]::GetFullPath($OutputDirectory)
} else {
    [IO.Path]::GetFullPath((Join-Path $workspaceRoot $OutputDirectory))
}
$runtimeNames = @('oo2core_9.dll', 'libzstd.dll', 'vcruntime140.dll')

# Prefer an explicitly supplied CRT. Otherwise use Visual Studio's x64 redist.
$crtSource = Join-Path $workspaceRoot 'lib/vcruntime140.dll'
if (-not (Test-Path -LiteralPath $crtSource -PathType Leaf)) {
    $crtCandidates = @()
    if ($env:VCToolsRedistDir) {
        $crtCandidates += Get-ChildItem -Path (Join-Path $env:VCToolsRedistDir 'x64/Microsoft.VC*.CRT/vcruntime140.dll') -File -ErrorAction SilentlyContinue
    }
    foreach ($programRoot in @($env:ProgramFiles, ${env:ProgramFiles(x86)}) | Where-Object { $_ }) {
        $crtPattern = Join-Path $programRoot 'Microsoft Visual Studio/*/*/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT/vcruntime140.dll'
        $crtCandidates += Get-ChildItem -Path $crtPattern -File -ErrorAction SilentlyContinue
    }
    $crt = $crtCandidates | Sort-Object { $_.VersionInfo.FileVersionRaw } -Descending | Select-Object -First 1
    if ($crt) {
        New-Item -ItemType Directory -Path (Join-Path $workspaceRoot 'lib') -Force | Out-Null
        Copy-Item -LiteralPath $crt.FullName -Destination $crtSource -Force
    }
}

foreach ($runtimeName in $runtimeNames) {
    $runtimeSource = Join-Path $workspaceRoot "lib/$runtimeName"
    if (-not (Test-Path -LiteralPath $runtimeSource -PathType Leaf)) {
        throw "Missing $runtimeSource. Place the runtime DLLs in lib/ before packaging."
    }
}

Push-Location -LiteralPath $workspaceRoot
try {
    & cargo build --release --locked -p soulkit
    if ($LASTEXITCODE -ne 0) { throw "Soulkit release build failed (exit $LASTEXITCODE)." }

    # Use Cargo's actual target directory, including CARGO_TARGET_DIR overrides.
    $metadataText = & cargo metadata --format-version 1 --no-deps --locked
    if ($LASTEXITCODE -ne 0) { throw 'Could not resolve the Cargo target directory.' }
    $cargoMetadata = $metadataText | ConvertFrom-Json
    $releaseRoot = Join-Path $cargoMetadata.target_directory 'release'
    $appSource = Join-Path $releaseRoot 'soulkit.exe'
    if (-not (Test-Path -LiteralPath $appSource -PathType Leaf)) {
        throw "The Windows release executable was not found at $appSource."
    }

    # Never clean an existing directory recursively or include exports/settings.
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    Copy-Item -LiteralPath $appSource -Destination (Join-Path $packageRoot 'soulkit.exe') -Force
    Copy-Item -LiteralPath (Join-Path $workspaceRoot 'LICENSE') -Destination (Join-Path $packageRoot 'LICENSE') -Force
    foreach ($runtimeName in $runtimeNames) {
        Copy-Item -LiteralPath (Join-Path $workspaceRoot "lib/$runtimeName") -Destination (Join-Path $packageRoot $runtimeName) -Force
    }
    Write-Host "Portable build: $packageRoot"
    Write-Host 'Local folder includes soulkit.exe, LICENSE and the three runtime DLLs. The desktop UI uses the system WebView2 runtime.'
} finally {
    Pop-Location
}
