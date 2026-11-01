<p align="center">
  <img src="soulkit/icons/android-chrome-512x512.png" width="180" height="180" alt="Soulkit logo: a green sprout peeking out of a toolbox" />
</p>

<h1 align="center">Soulkit</h1>

<p align="center">
  <strong>The Soulframe toolbox.</strong><br />
  <sub>Windows x64 · Built with Rust</sub>
</p>

<p align="center">
  <a href="#download-a-release">Download</a> ·
  <a href="#view-your-exported-data">View exports</a> ·
  <a href="#features">Features</a> ·
  <a href="#dll-setup">DLL setup</a> ·
  <a href="#run-soulkit">Getting started</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="#troubleshooting">Help</a>
</p>

---

Soulkit is a Windows desktop application for extracting and exploring assets
from an installed copy of Soulframe. Run complete extraction tasks, or browse
the game cache and export individual images, audio files, and static 3D models.
Explore the results in the built-in file library.

## View your exported data

Open the [Soulframe Package Viewer - Archive](https://cameronguthrie.github.io/soulframe-package-viewer/)
to browse and search `formatted_packages.json`, follow package references, and
inspect entries. Language exports resolve translated names and descriptions,
and matching exported images provide previews.

1. Click **Choose local files** → **Choose output folder** in the viewer.
2. Select your Soulkit output folder containing `formatted_packages.json`.
   The viewer also opens language exports in `Languages/` and supported images
   in `images/`.
3. Click **Done** to start browsing.

Use **Choose individual files** to select components separately. Files are
processed locally in your browser and are never uploaded. Select them again
after reloading the page. Use Soulkit's **File library** for other export types.

## Download a release

1. Open the [Releases page](https://github.com/CameronGuthrie/soulkit/releases)
   and choose the latest release.
2. Download the Windows x64 ZIP and extract it into a writable folder.
3. Follow the included `SETUP.md`, then open `soulkit.exe`.

> [!IMPORTANT]
> **DLLs are not included.** Supply your own compatible runtime DLLs before
> extracting assets. See [DLL setup](#dll-setup) for filenames and sources.

Each release includes a `SHA256SUMS.txt` file for verifying the downloaded ZIP.

## Features

**3D model extractor included:** the built-in [soulframe-models](soulframe-models/)
extractor exports static props, structures, and scenery as GLB files. Run it
from the **Extract** page, or select individual meshes in **Game cache**.
See [Extract 3D models](#extract-3d-models) for usage and support limits.

| Media | Game data |
| :--- | :--- |
| **Textures and images**<br />Interface graphics, artwork, textures, and world-map source tiles. Export supported images as PNG or DDS. | **Languages**<br />Localized text and language data for your selected language packs. |
| **Audio**<br />Voice lines, music, ambience, and sound effects, reconstructed as OGG or WAV where supported. | **Items and loot**<br />Item catalogues, crafting recipes, drop tables, and resolved loot data. |
| **Maps and dungeons**<br />Map tiles, dungeon floor plans, locations, and supporting map data. | **Daily calendars**<br />Zenith offers, weather, and Gods and Ghosts rotation data. |

### Explore before you export

- **Selective cache extraction:** search paths, open folders, preview an asset,
  and select exactly which images, audio, and static models to export.
  Selections can span folders and pages.
- **File library:** image thumbnails, audio playback, and previews for JSON,
  text, and other exported files. Collections provide shortcuts to common folders.
- **Activity:** live extraction output, cancellation, error details, and a
  copyable diagnostic report.
- **Settings:** configurable workspace, output, game-cache, and DLL folders;
  language selection; light and dark themes; and prompts for unsaved changes.

## What you need

To run Soulkit:

- A Windows x64 computer.
- A downloaded Soulframe installation with its game cache available.
- The [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).
  Choose the Evergreen Runtime; the standalone installer has an x64 option.
- The [Microsoft Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
- The DLL files described below.
- Free space on both the output drive and the workspace drive. Large selections
  can produce many files, and extraction can also need temporary working space.

Language-pack downloads require an internet connection. Items and loot
extraction may launch Soulframe to obtain fallback decryption keys; the app
shows this in the selected run before you start.

## DLL setup

Soulkit looks for these exact filenames in its configured DLL folder:

| File | Purpose | Where to get it |
| --- | --- | --- |
| `oo2core_9.dll` | Decompresses Oodle-compressed game-cache data. | A compatible **Windows x64 Oodle 9 runtime** from a game or SDK installation that supplies it. See the [Oodle publisher's site](https://www.radgametools.com/oodle.htm) for product and SDK information. |
| `libzstd.dll` | Decompresses Zstandard data used by language packs. | The **Windows 64-bit binary ZIP** from the [official Zstandard releases](https://github.com/meta/zstd/releases). Look for a `win64.zip` asset; its `dll/` folder contains `libzstd.dll`. |
| `vcruntime140.dll` | Supports the native Windows runtime libraries. | Install the [Microsoft Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist). An x64 installation normally places this file in `%WINDIR%\System32`. Developers can also obtain it from Visual Studio's x64 redistributable folder. |

The DLLs are not included in this source repository. A Soulframe installation
does **not necessarily include the separate Oodle or Zstandard DLLs**, so finding
the game cache does not mean that DLL setup is complete.

Some installations name the x64 Oodle 9 runtime `oo2core_9_win64.dll`. If that
is the compatible runtime you are using, copy it into Soulkit's DLL folder as
`oo2core_9.dll`. Keep the original file in its installation folder.

### Running a built copy

Put all three DLLs beside `soulkit.exe`:

```text
Soulkit/
  soulkit.exe
  oo2core_9.dll
  libzstd.dll
  vcruntime140.dll
```

Alternatively, open **Settings → Runtime DLLs**, choose a folder containing
all three files, and select **Save changes**. **Check files** refreshes the
list of found and missing files. Leaving the DLL folder blank uses the folder
containing `soulkit.exe`.

These checks confirm that files exist. They do not confirm that a DLL has the
correct architecture, version, or dependencies. Use the x64 versions for this app.

### Preparing DLLs for a source build

Create a `lib` folder beside the root `Cargo.toml` and place the DLLs there:

```text
lib/
  oo2core_9.dll
  libzstd.dll
  vcruntime140.dll
```

For Visual Studio installations, the x64 runtime DLL can be found under a path
such as:

```text
<Visual Studio>\VC\Redist\MSVC\<version>\x64\Microsoft.VC*.CRT\vcruntime140.dll
```

The packaging helper can find this Visual Studio copy automatically if
`lib/vcruntime140.dll` is missing. You must supply the two compression DLLs.

## Build from source

### Install the build tools

1. Install [Rust using rustup](https://rust-lang.org/tools/install/), choosing
   the Windows x64 MSVC toolchain (`x86_64-pc-windows-msvc`).
2. Install the [Microsoft C++ Build Tools](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup).
   Select **Desktop development with C++**, including the MSVC x64/x86 tools
   and a Windows SDK.
3. Open a new PowerShell window after installing the tools. Confirm that
   `cargo --version` and `rustc --version` work.

Download or clone the source, then open PowerShell in the repository folder
containing the root `Cargo.toml`. The first build downloads Cargo dependencies.
Node.js is optional and is used only for the UI regression tests.

### Build the application

After preparing `lib/`, run:

```powershell
cargo build --release --locked
```

The default build produces the GUI at `target/release/soulkit.exe`. It contains
the extraction engine and the web interface. Available DLLs from `lib/` are
copied beside the executable during the build.

Compilation can finish without the runtime DLLs, but tools that need those
libraries will fail until the DLLs are supplied.

### Create a portable application folder

From the repository root, run:

```powershell
.\soulkit\build-release.ps1
```

This builds the release app and copies it, together with `LICENSE` and the three
required DLLs, into `dist/Soulkit/` for your own local use. WebView2 and the Visual
C++ runtime must also be installed on the computer running it. Automated
releases package the app and setup instructions without any DLLs.

## Run Soulkit

Double-click `soulkit.exe`, or launch it from PowerShell:

```powershell
# Portable folder created by the packaging helper:
.\dist\Soulkit\soulkit.exe

# Direct Cargo build:
.\target\release\soulkit.exe
```

### First launch

1. Open **Settings** and choose a writable **Workspace folder**. Temporary
   extraction files use this location.
2. Choose an **Output folder** for the exported files and the file library.
3. Check **Runtime DLLs** and select another DLL folder if needed.
4. Check the detected **Game cache folder**, then select **Save changes**.

The usual cache location is:

```text
%LOCALAPPDATA%\Soulframe\Downloaded\Public\Cache.Windows
```

If your installation is elsewhere, select its `Cache.Windows` folder, the
parent `Public` folder, or the installation folder containing
`Downloaded\Public\Cache.Windows`. Let the launcher finish downloading the
game first. Keep the matching `Soulframe.x64.exe` in its installation alongside
the cache; audio reconstruction can need information from that executable.

## Choose what to extract

### Complete extraction tasks

On **Extract**, choose the tools you want and select **Run selected tools**.
The summary shows supporting tasks that need to run as well. For example, item
extraction can need language data, and calendars can need item and map data.
Use **Activity** to follow the run or stop it.

### Individual images, audio, and models

Open **Game cache** to browse indexed assets before exporting them:

1. Navigate through folders, or search the current folder and its subfolders.
2. Click a file for an image/audio preview or model export information.
   Use its checkbox to select it for export.
3. Select individual files, folder contents, or all search results. Selection
   includes matching results on other pages.
4. Choose PNG or DDS for supported images, then select **Extract selected**.

**Select whole game** selects all indexed **images, audio, and static models**
regardless of the current search or folder. **Rescan cache** reloads the index and clears
the selection. Previews use temporary app storage and do not export files to
your chosen output folder.

Some native textures and audio formats cannot yet be converted. Soulkit
preserves their original cache parts instead. In-app audio playback also
depends on the codecs available in WebView2.

### Extract 3D models

On **Extract**, choose **3D models** and select **Run selected tools**. To export
specific meshes, select them in **Game cache** and choose **Extract selected**.
GLB files are saved under `models/` in your output folder, preserving their
cache folder paths.

After building `soulkit-cli`, you can also run:

```powershell
.\target\release\soulkit-cli.exe models extract --output output/models
```

The standalone `soulframe-models` component provides the same extractor. See
[3D model commands](SOULKIT_COMMANDS.md#3d-models) for filtering and cache-path options.

Model extraction supports static meshes and writes `.glb` files. Animated and
skeletal meshes are excluded; meshes with morph targets are unsupported.
Open exported GLB files in compatible 3D software;
the app shows file information for models rather than an interactive 3D preview.

## Find your exported files

Exports go to the output folder chosen in Settings. The default is `output/`
under the detected workspace. Common locations include:

| Output | Contents |
| --- | --- |
| `images/` | Image exports; unsupported native textures use `images/native/`. |
| `audio/` | Audio exports and `index.json`; unsupported audio uses `audio/native/`. |
| `models/` | Static meshes exported as GLB files, retaining their cache folder paths. |
| `Languages/` | Extracted language JSON files. |
| `maps/` | Map images, dungeon floor plans, and map JSON data. |
| Output root | Item data, drop tables, and `droptables_resolved.json`. |
| `zenith-calendar.json` | Zenith calendar in the output root. |
| `gods-and-ghosts-calendar.json/` | Directory containing an index and calendars for the different unlock pools. |
| `weather/` | Weather calendar and guide. |

Open **File library** to browse the output, preview files, or reveal them in
Explorer. **Ctrl+K** focuses search; **Escape** closes a preview.

Successful full-pipeline runs clean up their owned temporary inputs in
`.soulkit-work/`. Failed runs keep them for diagnosis. **Keep temporary files**
retains them after successful runs too. App settings are stored separately
in the Windows application configuration folder.

## Troubleshooting

| Problem | What to check |
| --- | --- |
| Soulkit does not open | Install or repair the WebView2 Runtime and Microsoft Visual C++ x64 Redistributable. |
| Game cache is not found | Set the game-cache folder in Settings and make sure the game download is complete. |
| A DLL is missing or cannot load | Check the filenames and selected folder in Runtime DLLs. Use x64 files and install the Microsoft runtime; a file can be present while one of its dependencies is missing. |
| Extraction runs out of space | Check both the output and workspace drives. Available-space readings are not an estimate of the space a complete export needs. |
| A file will not preview or play | Some formats are preserved as native data, and some codecs are unavailable in WebView2. Check the preview notice or open the extracted file in another application. |
| An extraction fails | Open Activity, expand Technical details, and use Copy report to collect the diagnostic report and run output. |

## Command-line tools and development checks

The GUI is the default Cargo target. Build the optional command-line app or
an individual component explicitly:

```powershell
cargo build --release --locked -p soulkit-cli
cargo build --release --locked -p soulframe-audio
cargo build --release --locked -p soulframe-images
cargo build --release --locked -p soulframe-models
```

See [SOULKIT_COMMANDS.md](SOULKIT_COMMANDS.md) for command-line usage and
the [resolved-loot schema](soulkit-cli/docs/resolved-loot.md) for that export's
input and output formats.

Run the Rust checks with:

```powershell
cargo test --workspace --locked
```

With Node.js installed, run the UI checks with:

```powershell
node soulkit/tests/settings-ui.cjs
node soulkit/tests/errors-ui.cjs
node soulkit/tests/cache-browser-ui.cjs
```

## License

Soulkit is licensed under the [GNU Affero General Public License version 3.0](LICENSE)
(`AGPL-3.0-only`).
