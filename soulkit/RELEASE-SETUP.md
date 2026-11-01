# Run Soulkit

Extract the ZIP into a writable folder on a Windows x64 computer, then open
`soulkit.exe`. The extraction engine is built into the application; no separate
command-line tools are required.

## Prerequisites

Install the [Microsoft Edge WebView2 Evergreen Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)
and the [Microsoft Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
Soulkit also needs a downloaded Soulframe installation and enough free space
for exports and temporary extraction files.

## Supply your own runtime DLLs

**No DLLs are included in this release.** Obtain compatible Windows x64 copies
from your own installations or the publishers' official distributions:

| Filename | Source |
| --- | --- |
| `oo2core_9.dll` | A game or SDK installation supplying a compatible Oodle 9 runtime. The [Oodle publisher's site](https://www.radgametools.com/oodle.htm) describes the product and SDK. Soulframe does not necessarily include this separate DLL. If your compatible x64 copy is called `oo2core_9_win64.dll`, copy it as `oo2core_9.dll` into Soulkit's DLL folder. |
| `libzstd.dll` | The Windows 64-bit ZIP from the [official Zstandard releases](https://github.com/meta/zstd/releases), inside its `dll` folder. |
| `vcruntime140.dll` | The installed Microsoft Visual C++ x64 Redistributable, normally in `%WINDIR%\System32`, or Visual Studio's x64 redistributable folder. |

Place the three files beside `soulkit.exe`, or open **Settings → Runtime DLLs**
and select a folder containing them. Select **Check files**, then **Save changes**.
The checks confirm presence; DLL versions, architecture and dependencies must
also be compatible. Keep the original files in their installation folders.

## First launch

1. Open **Settings** and choose writable workspace and output folders.
2. Confirm the game cache location. The usual location is
   `%LOCALAPPDATA%\Soulframe\Downloaded\Public\Cache.Windows`.
3. Configure the runtime DLL folder and save changes.
4. Use **Extract** for complete tasks, or **Game cache** to select individual
   images, audio, and static models. Browse exported files in **File library**.

Use **Activity → Technical details → Copy report** if an extraction fails.

## Which build is this?

`main` releases contain the full feature set. `public` releases contain the
trimmed public feature set. The version and source commit are recorded in
`BUILD-INFO.json`. Automatic push builds are published as prereleases.

## License

Soulkit is licensed under the GNU Affero General Public License version 3.0
(`AGPL-3.0-only`). The full terms are in `LICENSE` beside `soulkit.exe`.
