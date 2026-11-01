# Resolved loot exports

`soulkit-cli run-all` (also the no-argument default) writes
`droptables_resolved.json` to its items output directory. The standalone command
reads existing inputs:

```powershell
soulkit-cli resolve-loot --packages output/formatted_packages.json --decrypted-dir decrypted_raw --output output
soulkit-cli resolve-loot --levels 15,20,25,35 --languages output/Languages_en.json --exe C:/path/to/Soulframe.x64.exe
```

The export contains gameplay data only. There are no local file paths, hashes,
pointers, offsets, script references, research documents, or repeated diagnostic
scenarios. Game resource paths remain stable IDs for joining data elsewhere.
Probabilities are fractions between 0 and 1; unknown values are `null`.

## `droptables_resolved.json`

`tables` uses the earlier `table_name` / `pools` / `items` structure. Empty
categories are omitted; each pool retains its original `category` index.

```json
{
  "probability_unit": "fraction",
  "probability_scope": "conditional_item_selection",
  "drop_chance_basis": "unmodified_category",
  "tables": [
    {
      "table_name": "Example",
      "path": "/SF/Types/PickUps/DropTables/ExampleDropTable",
      "pool_count": 1,
      "total_items": 1,
      "pools": [
        {
          "category": 7,
          "drop_chance": 0.8,
          "rolls": {"min": 0, "max": 1},
          "quality_tiers": [2, 3, 4, 5, 6],
          "items": [
            {
              "path": "/SF/ExampleItem",
              "name": "Example item",
              "rarity": "Uncommon",
              "bias": 0.0,
              "min_level": 15,
              "max_level": 100,
              "probability": null
            }
          ]
        }
      ]
    }
  ],
  "sources": []
}
```

The example is illustrative. `rolls` counts category selections, not units in a
pickup stack. `drop_chance` is the base chance for each optional trial above the
minimum. `quality_tiers` retains the category's authored quality-tier IDs.

`min_level` and `max_level` are inclusive generation-level bounds. Overlapping
ranges remain separate entries and can both be eligible. `rarity` is the authored
rarity label. The item selection calculation uses known package resource rarity;
unknown resource inputs do not fall back to guessed probabilities.

When a conditional selection probability is available for a sampled level, an
item also has `probabilities_by_level`, for example
`[{"level": 35, "probability": 0.25}]`. Other levels remain unknown; these rows
must not be interpolated or extended to an entire range. `probability` stays null
where a single level-independent probability has not been established. These
probabilities assume resource acceptance, default category interpolation, and
uniform selection draws; they are not per-enemy acquisition probabilities.

`sources` joins resource IDs to tables. Each row contains `path`, `name`, and
`event` (`drop`, `catch`, `release`, `collect`, `spawn`, `open`, `destroy`, or a
conditional `cargo_open`, `cargo_destroy`, `cargo_recovery`). Ordinary rows have
`table`. Mode-dependent rows have `tables: {"normal": ..., "cogah": ...}`.
Chest rows also list `enemies`; region override rows have `region`. A null Cogah
chest table remains null, with no normal fallback. Unsupported tables and tables
without a proven resource identity are omitted from `tables`. An authored source
can still reference a table ID whose contents are unavailable in the snapshot.

Direct and region routes now include `authored_drop_chance` and `enabled`.
`enabled: false` means the authored drop chance is zero; an inherited table
reference alone does not make that a live reward channel. Unknown chance stays
null. Chest `enemy_triggers` distinguishes death, Rider pre-death, and Corrupted
Dog purification routes. A missing pickup resource is not replaced with a parent
or similarly named asset.

`source_groups` lists sources configured with more than one distinct table. Each
row has `path`, `name`, `table_count`, `configured_table_count`, and `routes`.
`table_count` excludes explicitly disabled routes; unknown routes are retained.
`configured_table_count` also counts disabled table references. The same table
referenced repeatedly counts once. Each route provides the table ID, event,
configuration-enabled state, and applicable `mode`, `region`, or intermediary
pickup (`via`). Regional replacements and their fallback are marked with
`selection`. These are available conditional routes, not a count of tables rolled
simultaneously. In particular, normal/Cogah alternatives must not be summed.
This inventory covers the supported, traced routes; it is not exhaustive.

Ordinary chest and destructible routes retain their table and intermediary
action where applicable. Ode Drone cargo routes retain `cargo_index`,
`cargo_selection_probability`, `conditions`, and the physics object in `via`.
`cargo_probability_basis: "normalized_weights"` identifies the cargo-choice
model; this is separate from item odds. Cargo recovery requires unchanged cargo
to move too far and grants rewards only to eligible nearby players. It is not an
additional unconditional death roll. Siege mine-laying drones remove their cargo.
`spawn_chance` and `max_reward_distance` retain the authored script values.

`weapon_resolution` describes the generic weapon pickup's conditional identity
rule. The native spawn path tries the context avatar's selected weapon, then
inventory slots 5 and 1, stopping at the first compatible weapon. Its
`StoreItemDrop` supplies the item; an absent mapping yields no weapon item.
This applies when the pickup has no assigned item. Mushroom routes explicitly
set `weapon_context: "collector"`; other untraced callers are not automatically
classified as player or enemy. The rule and context annotation are withheld for
an unverified executable. Generic entries remain generic in `tables`, with no
invented global weapon pool or added named-item odds.

`observations` is included only when `--invocations` is supplied. It contains
capture completeness, observed counts, compact calls, and enemy/run conditional
item probabilities. Research and capture-file references are omitted.

## Observed invocation evidence

Default reports leave actual call counts and aggregate acquisition probabilities
null. The game is not observed merely by decoding a table. `--invocations FILE`
imports an existing trace with resolved native call context. The caller supplies
evidence; the exporter validates structure and matching hashes, not whether a
human's evidence description is true. This command does not create that trace.

The JSON shape is below. Angle-bracket strings are descriptions to replace with
real evidence, not valid runtime facts. Include all eight category contexts per
call, including empty categories, and every entry needed by eligible groups.

```json
{
  "schema_version": 1,
  "native_sha256": "<supported executable SHA-256>",
  "packages_sha256": "<exact formatted_packages.json SHA-256>",
  "evidence_source": "<capture identity and how calls were observed>",
  "capture_complete": true,
  "independent_uniform_draws_model": false,
  "calls": [
    {
      "invocation_id": "<unique native table call>",
      "run_id": "<observed run>",
      "enemy_id": null,
      "selected_table": "/SF/<actual selected resource after swaps>",
      "table_sha256": "<matching raw file SHA-256>",
      "selection_evidence": "<observation of selected table and mode/region>",
      "generation_level": 35,
      "categories": [
        {
          "category_index": 0,
          "effective_chance": 0.5,
          "context_evidence": "<post-upgrade chance and native call arguments>",
          "selection_with_replacement": true,
          "selector": {
            "generation_level": 35,
            "random_selection_mode": true,
            "category_interpolation": {
              "minimum_level": 0.0,
              "maximum_level": 100.0,
              "bias_start": 0.0,
              "bias_end": 0.0,
              "flip_start": 0.0,
              "flip_end": 0.0
            },
            "entry_evidence": {
              "123": {
                "accepted": true,
                "runtime_rarity": 0,
                "source": "<native resource/filter/rarity observation at file offset 123>"
              }
            },
            "assumptions": []
          }
        }
      ]
    }
  ]
}
```

`category_interpolation` can be null only when all eligible groups supply their
own interpolation. Unknown entry acceptance or rarity withholds normalization;
proven exclusions are represented with `accepted: false`. All-items/force-all
paths remain unsupported by the random selector. `enemy_id` is optional for a
non-enemy call; identities are grouped within `run_id`.

Observed call counts are exported even for incomplete captures (as lower bounds).
Aggregates require a complete capture, all resolved contexts, stable repeated
selection with replacement, and an explicitly chosen independent-uniform-draws
model. Native successive LCG draws do not themselves establish independence.
Duplicate entries are summed by resource after native weighting, not removed
before normalization.

For direct-entry per-selection mass `q`, strict-U24 category trial mass `p`,
minimum `m`, and maximum `M`, the declared model uses:

```text
P(no item from category) = (1 - q)^m * (1 - p*q)^(M-m)
P(at least one across supplied calls) = 1 - product(P(no item))
```

The result describes those observed contexts. It does not forecast spawn counts,
expand nested tables/randomized weapon identities, equate pickup stacks with
category counts, or infer calls from a source binding.
