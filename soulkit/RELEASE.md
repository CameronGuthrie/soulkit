# Soulkit releases

## Download and run

Open the repository's [Releases page](https://github.com/CameronGuthrie/soulkit/releases)
and choose the branch you want:

| Release channel | Source branch | Version example |
| --- | --- | --- |
| Full | `main` | `0.1.0-main.42` |
| Public | `public` | `0.1.0-public.43` |

The channels keep their own feature sets. Automatic builds are marked as
prereleases. Each release identifies its source commit and includes a Windows
x64 ZIP named `Soulkit-<version>-windows-x64.zip`, plus `SHA256SUMS.txt` for
checking the download.

Extract the ZIP into a writable folder. It contains `soulkit.exe`, `SETUP.md`,
`BUILD-INFO.json`, and the AGPL-3.0 license in `LICENSE`. The GUI includes its
extraction engine and interface; you do not need to download separate
command-line or per-tool executables.

**DLL files are not included in releases.** Supply your own compatible Windows
x64 copies before running extraction tools that need them.

## Runtime setup

Follow the [runtime setup guide](./RELEASE-SETUP.md), included as `SETUP.md` in
each ZIP. It links the official WebView2 and Visual C++ prerequisites and
explains where to obtain `oo2core_9.dll`, `libzstd.dll`, and `vcruntime140.dll`.
You can put your DLLs beside `soulkit.exe` or select their folder in
**Settings → Runtime DLLs**. The guide also covers first-launch settings.

To check a downloaded archive in PowerShell:

```powershell
Get-FileHash .\Soulkit-0.1.0-public.43-windows-x64.zip -Algorithm SHA256
```

Compare the hash with the matching entry in `SHA256SUMS.txt` from that release.

## Automatic builds

Pushes to `main` and `public` run the GitHub Actions release workflow. It tests
and builds the branch's Soulkit GUI, packages only the application and setup
information, and publishes a separate prerelease after the checks succeed.
DLLs, game files, exports, and local settings are excluded from the archive.
The workflow can also be started manually for either supported branch from
the **Actions** tab.

Keep `.github/workflows/` identical on `main` and `public`. When changing a
workflow, let existing releases finish, then update `main` and `public` together.
GitHub's built-in token can reject release creation if the target commit's
workflow files differ from the current default branch; see the
[release API permissions](https://docs.github.com/en/rest/releases/releases#create-a-release).

The base application version lives in `[workspace.package].version` in the
root `Cargo.toml`. Soulkit and Soulkit CLI inherit it. For an automatic build,
the workflow adds `-<branch>.<run-number>` in its temporary checkout, so the
version shown by the app matches its release. For example, base `0.1.0` on
`main` in run 42 becomes `0.1.0-main.42`, tagged `v0.1.0-main.42`. This does
not create version-bump commits on either branch.

To change the base version, run this from the repository root:

```powershell
.\soulkit\set-version.ps1 -Version 0.2.0
.\soulkit\set-version.ps1 -Check
```

Commit the updated manifest and lockfile on each branch that should use the
new base version. A normal source build uses that base version directly:

```powershell
cargo build --release --locked
```

Compilation does not require the runtime DLLs. Extraction tools that use
those libraries require them when the app runs. The local portable-folder
helper can copy your DLLs into a folder for your own use; the automated
release workflow never packages them.
