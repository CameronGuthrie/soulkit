# Keep the GUI, embedded CLI, and lockfile on the workspace app version.
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$utf8 = [Text.UTF8Encoding]::new($false)
$versionPattern = '(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:main|public)\.(?:0|[1-9][0-9]*))?'
if ($Version -and $Version -cnotmatch "\A$versionPattern\z") {
    throw 'Version must be MAJOR.MINOR.PATCH, optionally followed by -main.NUMBER or -public.NUMBER, without leading zeroes.'
}
if ($Check -and $Version) {
    throw 'Use -Check to validate the current version, or -Version to change it.'
}

$manifestPath = Join-Path $workspaceRoot 'Cargo.toml'
$lockPath = Join-Path $workspaceRoot 'Cargo.lock'
$manifest = [IO.File]::ReadAllText($manifestPath)
$lock = [IO.File]::ReadAllText($lockPath)
$workspacePattern = '(?m)(^\[workspace\.package\]\r?\n)(version = ")([^"\r\n]+)(")'
$workspaceMatch = [regex]::Match($manifest, $workspacePattern)
if (-not $workspaceMatch.Success) { throw 'Expected version as the first field in [workspace.package].' }
$currentVersion = $workspaceMatch.Groups[3].Value
if ($currentVersion -cnotmatch "\A$versionPattern\z") { throw "Invalid workspace app version: $currentVersion" }

foreach ($package in @('soulkit', 'soulkit-cli')) {
    $packageManifest = [IO.File]::ReadAllText((Join-Path $workspaceRoot "$package/Cargo.toml"))
    if ($packageManifest -notmatch '(?m)^version\.workspace = true\r?$') {
        throw "$package must inherit its version from [workspace.package]."
    }
    $packagePattern = '(?m)(^name = "' + [regex]::Escape($package) + '"\r?\nversion = ")([^"\r\n]+)(")'
    $packageMatches = [regex]::Matches($lock, $packagePattern)
    if ($packageMatches.Count -ne 1 -or $packageMatches[0].Groups[2].Value -cne $currentVersion) {
        throw "Cargo.lock must contain exactly one $package entry at version $currentVersion."
    }
}
$tauriConfig = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'tauri.conf.json')) | ConvertFrom-Json
if ($null -ne $tauriConfig.PSObject.Properties['version']) {
    throw 'Remove version from tauri.conf.json so Tauri inherits the Cargo package version.'
}

if ($Version) {
    $manifest = [regex]::Replace($manifest, $workspacePattern, {
        param($match)
        $match.Groups[1].Value + $match.Groups[2].Value + $Version + $match.Groups[4].Value
    })
    foreach ($package in @('soulkit', 'soulkit-cli')) {
        $packagePattern = '(?m)(^name = "' + [regex]::Escape($package) + '"\r?\nversion = ")([^"\r\n]+)(")'
        $lock = [regex]::Replace($lock, $packagePattern, {
            param($match)
            $match.Groups[1].Value + $Version + $match.Groups[3].Value
        })
    }
    [IO.File]::WriteAllText($manifestPath, $manifest, $utf8)
    [IO.File]::WriteAllText($lockPath, $lock, $utf8)
    Write-Host "Soulkit version set to $Version."
} else {
    Write-Host "Soulkit version $currentVersion is consistent."
}
