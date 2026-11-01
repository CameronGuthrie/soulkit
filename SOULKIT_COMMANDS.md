# Soulkit command reference

Build the optional command-line app from the workspace root:

```powershell
cargo build --release --locked -p soulkit-cli
.\target\release\soulkit-cli.exe --help
```

The examples below use `soulkit-cli` for this executable. Paths are relative to
the current working directory. Use `<command> --help` for all available options.
The GUI embeds the same engine and does not need a separate CLI executable.

## Full pipeline

```powershell
soulkit-cli run-all
soulkit-cli run-all --locales en,fr --skip-audio
soulkit-cli run-all --calendar-from 2026-11-01 --calendar-days 30
soulkit-cli run-all --keep-intermediates
```

Running without a subcommand also runs the full pipeline. Final output defaults
to `output/`. Default calendars cover 90 days before through 90 days after the
run's UTC date. Gods and Ghosts exports an index and one file per unlock pool.

The pipeline prepares languages, package and reward inputs, drop tables,
interface images, audio, maps, item catalogues, loot exports and calendars.
Module flags include `--skip-languages`, `--skip-droptables`, `--skip-images`,
`--skip-audio`, `--skip-map`, `--skip-items`, `--skip-calendars` and
`--skip-resolved-loot`. Supporting inputs are still required by retained stages;
see `run-all --help` for output-path and input-path overrides.

Temporary inputs use `.soulkit-work/run-*`. Successful runs clean that owned
folder; failures and `--keep-intermediates` retain it. Explicit
`--keys-output`, `--decrypted-output` and `--items-manifests` paths are retained.
Keys and decrypted data should stay outside source control.

## Images and audio

```powershell
soulkit-cli images extract --filter png --path-filter SF/Interface/ --output output/images --output-format png
soulkit-cli images extract --list-all --filter swf --path-filter Interface --path-filter-substring --output output/images
soulkit-cli audio extract
soulkit-cli audio extract --list-paths --path-filter Music
soulkit-cli audio extract --path-filter Sounds/Dialog --limit 20 --output output/dialogue
```

`--list-all` permits image and interface SWF assets across cache families; it
is not an arbitrary cache-file exporter. For exact individual image/audio
selection and previews, use the GUI's Game cache page.

Image and audio extractors accept `--cache-path <Cache.Windows>`. Automatic
cache detection uses `SOULFRAME_ROOT` (the game's `Downloaded/Public` folder)
and the default local installation. Set `SOULFRAME_OODLE_LIB` or
`SOULFRAME_ZSTD_LIB` to full DLL paths when needed outside the packaged app.

Audio preserves common and localized cache families under its output folder and
writes `index.json`. Unsupported formats retain native cache parts. Vorbis
reconstruction reads codebooks from the matching installed game executable;
use `audio extract --game-exe <Soulframe.x64.exe>` for a copied cache.

## 3D models

```powershell
soulkit-cli models extract --output output/models
soulkit-cli models extract --list-paths --path-filter SF/Objects/
soulkit-cli models extract --path-filter SF/Objects/ --limit 20 --output output/models
```

The model extractor exports supported static meshes as GLB files. Animated and
skeletal meshes are excluded, and morph targets are unsupported. It accepts
`--cache-path <Cache.Windows>` and uses
the same Oodle runtime as the image extractor. In the GUI, select **3D models**
on Extract or choose individual meshes in Game cache. The model task runs
separately from `run-all`; export GLB files to open in compatible 3D software.

## Maps and languages

```powershell
soulkit-cli map extract --output output/maps
soulkit-cli map extract --skip-dungeons --output output/maps
soulkit-cli languages download --locales en,fr
soulkit-cli languages extract --locales en,fr
soulkit-cli languages extract-from-cache --cache-dir C:/path/to/Cache.Windows --locales en
```

Map exports include level tiles, map JSON and dungeon room floor plans from
navmeshes. `--packages <formatted_packages.json>` supplies localized map names.
Standalone language commands keep their own output defaults; the full pipeline
publishes language JSON under `output/Languages/`.

## Items, drop tables and loot

```powershell
soulkit-cli droptables run-all
soulkit-cli items all-items --packages output/packages_bin_attrs.json
soulkit-cli items format-packages --packages output/packages_bin_attrs.json
soulkit-cli items reward-distributions --manifests output/manifest_rewards_raw.json
soulkit-cli resolve-loot --packages output/formatted_packages.json --decrypted-dir decrypted_raw --output output
```

The full pipeline prepares the required item inputs automatically. Standalone
commands require those inputs to exist. Drop-table extraction uses cache-derived
keys first and can launch the game to refresh fallback keys when necessary.

Resolved loot writes `droptables_resolved.json`. Unknown probabilities remain
`null`; sampled level probabilities do not establish a rate for every level or
enemy. See the
[loot input and export schema](soulkit-cli/docs/resolved-loot.md), including the
optional `--invocations` input format.

## Daily calendars

```powershell
soulkit-cli zenith-offers --packages output/formatted_packages.json --json
soulkit-cli zenith-calendar --from 2026-11-01 --days 30 --output output/zenith-calendar.json
soulkit-cli gods-and-ghosts-calendar --from 2026-11-01 --days 30 --all-unlock-variations --output output/gods-and-ghosts
soulkit-cli weather-calendar --from 2026-11-01 --days 7 --output output/weather/weather-calendar.json
```

Calendars use extracted package/map definitions and the current game's rotation
algorithms. Content updates can change predictions; regenerate inputs after a
patch. Standalone commands show their required/default paths in `--help`.
