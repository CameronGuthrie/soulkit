//! Evidence-preserving exports; category selection is not an enemy death count.
use anyhow::{bail, ensure, Context, Result};
use clap::Args as ClapArgs;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use soulframe_droptables::resolved_raw::{decode_categories, DecodedDropTable};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_LEVELS: &[u32] = &[1, 10, 11, 15, 20, 25, 30, 31, 35, 50, 51, 100];
const NATIVE_SHA256: &str = "e90536169d0ee4ce9222b0a90d7e9bf071d306f5f53d9be7a74cf63ab4a9d00b";
const LOOT_CHEST: &str = "/SF/Scripts/Levels/LootChest.lua";
const ZONE_OVERRIDE: &str = "/SF/Scripts/ZoneSpawnDropTableOverride.lua";

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Structured package resources; never the flattened drop-table percentages.
    #[arg(long, default_value = "output/formatted_packages.json")]
    pub packages: PathBuf,
    #[arg(long, default_value = "decrypted_raw")]
    pub decrypted_dir: PathBuf,
    /// Directory for droptables_resolved.json.
    #[arg(long, default_value = "output")]
    pub output: PathBuf,
    /// Executable used to validate recovered native formulas (defaults to installed client).
    #[arg(long)]
    pub exe: Option<PathBuf>,
    /// Sample unsigned generation inputs, not player ranks. Full authored ranges are always exported.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "1,10,11,15,20,25,30,31,35,50,51,100"
    )]
    pub levels: Vec<u32>,
    /// Optional flat English localization JSON.
    #[arg(long)]
    pub languages: Option<PathBuf>,
    /// Optional observed invocation contexts; see docs/resolved-loot.md for the schema.
    #[arg(long)]
    pub invocations: Option<PathBuf>,
    #[arg(skip)]
    pub input_context: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_json(path: &Path) -> Result<(Value, String)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?,
        digest(&bytes),
    ))
}

fn pointer(parts: &[&str]) -> String {
    format!(
        "/{}",
        parts
            .iter()
            .map(|s| s.replace('~', "~0").replace('/', "~1"))
            .collect::<Vec<_>>()
            .join("/")
    )
}

/// Resolve a nested field independently through resource inheritance. Explicit null
/// or a non-object at an intermediate field blocks inheritance; absent keys do not.
fn inherited(packages: &Value, resource: &str, keys: &[&str]) -> Option<(Value, String)> {
    let mut current = resource;
    let mut visited = BTreeSet::new();
    while visited.insert(current) {
        let record = packages.get(current)?;
        let mut cursor = record.get("attributes");
        let mut missing = false;
        for (index, key) in keys.iter().enumerate() {
            match cursor {
                Some(Value::Object(map)) => {
                    cursor = map.get(*key);
                    if cursor.is_none() {
                        missing = true;
                        break;
                    }
                }
                Some(_) => {
                    let mut parts = vec![current, "attributes"];
                    parts.extend(&keys[..index]);
                    return Some((Value::Null, pointer(&parts)));
                }
                None => {
                    missing = true;
                    break;
                }
            }
        }
        if !missing {
            if let Some(value) = cursor {
                let mut parts = vec![current, "attributes"];
                parts.extend(keys);
                return Some((value.clone(), pointer(&parts)));
            }
        }
        current = record.get("parent")?.as_str()?;
    }
    None
}

fn field(packages: &Value, resource: &str, keys: &[&str]) -> Value {
    inherited(packages, resource, keys)
        .map(|(value, _)| value)
        .unwrap_or(Value::Null)
}

fn field_evidence(packages: &Value, resource: &str, keys: &[&str]) -> Value {
    match inherited(packages, resource, keys) {
        Some((value, at)) => json!({"value":value,"package_pointer":at}),
        None => {
            json!({"value":null,"package_pointer":null,"status":"not_authored_or_missing_parent"})
        }
    }
}

fn source_name(packages: &Value, languages: &Value, resource: &str) -> String {
    for key in ["LocTag", "LocalizeTag", "Name"] {
        if let Some(tag) = field(packages, resource, &[key]).as_str() {
            if let Some(label) = languages.get(tag).and_then(Value::as_str) {
                return label.into();
            }
        }
    }
    resource.rsplit('/').next().unwrap_or(resource).to_owned()
}

fn is_resource(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.starts_with("/SF/") || s.starts_with("/Lotus/"))
}

fn mushroom_enemy_binding(packages: &Value, resource: &str, pickup: &str) -> Option<Value> {
    let death = field(packages, resource, &["OnDeathScript", "_mushroomPickup"]);
    if death.as_str() == Some(pickup)
        && field(packages, resource, &["OnDeathScript", "Script"])
            == "/SF/Scripts/Npc/SubbossRednightResourceSpawn.lua"
        && matches!(
            field(packages, resource, &["OnDeathScript", "Function"]).as_str(),
            Some("OnSubBossDeath" | "SpawnShroomRing")
        )
    {
        return Some(
            json!({"pickup":field_evidence(packages, resource, &["OnDeathScript", "_mushroomPickup"]),"trigger":"death"}),
        );
    }
    // The mounted Rider uses its own pre-death handler, which delegates to the
    // same mushroom ring helper only when the authored switch is enabled.
    let predeath = field(packages, resource, &["OnPreDeathScript", "_mushroomPickup"]);
    let enabled = field(
        packages,
        resource,
        &["OnPreDeathScript", "_shouldSpawnMushroomRing"],
    );
    if predeath.as_str() == Some(pickup)
        && field(packages, resource, &["OnPreDeathScript", "Script"])
            == "/SF/Scripts/Npc/MendicantRider.lua"
        && field(packages, resource, &["OnPreDeathScript", "Function"]) == "OnRiderDeath"
        && (enabled == 1 || enabled == true)
    {
        return Some(
            json!({"pickup":field_evidence(packages, resource, &["OnPreDeathScript", "_mushroomPickup"]),"trigger":"pre_death"}),
        );
    }
    // The Corrupted Dog boss's purification entity owns the mushroom settings.
    // Relative entity references are resolved at the resource that authors them.
    const DOG_SCRIPT: &str = "/SF/Types/Enemies/Sinecure/OdeAttackDog/DogScripts.lua";
    if field(packages, resource, &["OnPreDeathScript", "Script"]) == DOG_SCRIPT
        && field(packages, resource, &["OnPreDeathScript", "Function"]) == "OnCorruptedPreDeath"
    {
        let entity = authored_resource(
            packages,
            resource,
            &["OnPreDeathScript", "_dogPurifySseType"],
        )?;
        let enabled = field(packages, &entity, &["Script", "_shouldSpawnMushroomRing"]);
        if field(packages, &entity, &["Script", "Script"]) == DOG_SCRIPT
            && field(packages, &entity, &["Script", "Function"]) == "OnPreDeathSteal"
            && field(packages, &entity, &["Script", "_mushroomPickup"]).as_str() == Some(pickup)
            && (enabled == 1 || enabled == true)
        {
            return Some(
                json!({"pickup":field_evidence(packages,&entity,&["Script","_mushroomPickup"]),"trigger":"purification","entity":entity}),
            );
        }
    }
    None
}

fn authored_resource(packages: &Value, resource: &str, keys: &[&str]) -> Option<String> {
    let (value, at) = inherited(packages, resource, keys)?;
    let path = value.as_str()?;
    if path.starts_with('/') {
        return Some(path.into());
    }
    if path.is_empty() || path.contains("..") || path.contains('\\') {
        return None;
    }
    let owner = at.split('/').nth(1)?.replace("~1", "/").replace("~0", "~");
    let (directory, _) = owner.rsplit_once('/')?;
    Some(format!("{directory}/{path}"))
}

fn source_bindings(packages: &Value, languages: &Value) -> Vec<Value> {
    let mut bindings = Vec::new();
    let Some(records) = packages.as_object() else {
        return bindings;
    };
    for resource in records.keys() {
        if !resource.starts_with("/SF/") {
            continue;
        }
        for key in ["DropTable", "CatchDropTable", "ReleaseDropTable"] {
            let table = field(packages, resource, &[key]);
            if is_resource(&table) {
                bindings.push(json!({"kind":"authored_table_binding", "source":resource,
                    "source_name":source_name(packages,languages,resource),"field":key,
                    "table":field_evidence(packages,resource,&[key]),
                    "authored_drop_chance":if key=="DropTable" {field(packages,resource,&["DropChance"])}else{Value::Null},
                    "actual_invocations_per_enemy":null,
                    "per_enemy_probability":null,"reason":"binding does not prove when or how many times the table is invoked"}));
            }
        }
        if field(packages, resource, &["PickUpScript", "Script"]) == LOOT_CHEST
            && field(packages, resource, &["PickUpScript", "Function"]) == "MushroomChest"
        {
            let normal = field_evidence(packages, resource, &["PickUpScript", "_dropTable"]);
            let hard = field_evidence(packages, resource, &["PickUpScript", "_hardmodeDroptable"]);
            if is_resource(&normal["value"]) || is_resource(&hard["value"]) {
                let enemies: Vec<_> = records
                    .keys()
                    .filter_map(|r| {
                        mushroom_enemy_binding(packages,r,resource).map(|binding|
                            json!({"source":r,"source_name":source_name(packages,languages,r),"pickup":binding["pickup"],"trigger":binding["trigger"]}))
                    })
                    .collect();
                bindings.push(json!({"kind":"mushroom_chest_mode_swap", "source":resource,
                    "source_name":source_name(packages,languages,resource),"enemy_bindings":enemies,
                    "normal_table":normal,"hard_mode_table":hard,
                    "selection_time":"collection","selection_condition":"IsHardModeActive",
                    "selection":"mutually_exclusive","hard_mode_null_behavior":"no_normal_fallback",
                    "script_evidence":{"script":LOOT_CHEST,"prototype":19,"normal_pcs":[70,74],"hard_pcs":[76,87]},
                    "helper_calls_per_successful_branch":1,"actual_native_table_invocations_per_enemy":null,
                    "per_enemy_probability":null,"reason":"collection, player/authority branches, duplicate guards and native invocation cardinality must be observed"}));
            }
        }
        if field(packages, resource, &["ScriptInstance", "Script"]) == ZONE_OVERRIDE
            && field(packages, resource, &["ScriptInstance", "Function"]) == "OnSpawn"
        {
            let regions = field(packages, resource, &["ScriptInstance", "_regionLevelInfos"]);
            let normal = field(
                packages,
                resource,
                &["ScriptInstance", "_dropTableOverrides"],
            );
            let hard = field(
                packages,
                resource,
                &["ScriptInstance", "_hardModeDropTableOverrides"],
            );
            if let Some(regions) = regions.as_array() {
                for (index, region) in regions.iter().enumerate() {
                    let base = normal.get(index).cloned().unwrap_or(Value::Null);
                    let replacement = hard.get(index).filter(|v| is_resource(v)).cloned();
                    bindings.push(json!({"kind":"region_table_override", "source":resource,
                        "source_name":source_name(packages,languages,resource),"region_level_info":region,
                        "authored_drop_chance":field(packages,resource,&["DropChance"]),
                        "normal_table":base,"hard_mode_table":replacement.clone().unwrap_or(base.clone()),
                        "has_hard_mode_replacement":replacement.is_some(),"selection_time":"OnSpawn",
                        "selection":"parallel_array_region_identity_then_non_null_hard_overlay",
                        "region_array":field_evidence(packages,resource,&["ScriptInstance","_regionLevelInfos"]),
                        "normal_array":field_evidence(packages,resource,&["ScriptInstance","_dropTableOverrides"]),
                        "hard_array":field_evidence(packages,resource,&["ScriptInstance","_hardModeDropTableOverrides"]),
                        "hard_mode_aura":field_evidence(packages,resource,&["ScriptInstance","_hardModeDebuffAuraUpgrade"]),
                        "script_evidence":{"script":ZONE_OVERRIDE,"prototype":0,"pcs":[20,71]},
                        "actual_invocations_per_enemy":null,"per_enemy_probability":null}));
                }
            }
        }
    }
    bindings
}

/// Count U24 values passing the native strict comparison, including f32 rounding.
fn quantity_trial_probability(chance: f32) -> f64 {
    let n = 1u32 << 24;
    let mut lo = 0u32;
    let mut hi = n;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if (mid as f32 / n as f32) < chance.clamp(0.0, 1.0) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    f64::from(lo) / f64::from(n)
}

fn quantity_model(category: &soulframe_droptables::resolved_raw::DropCategory) -> Value {
    let trials = category.maximum_drops - category.minimum_drops;
    let p = quantity_trial_probability(category.chance);
    json!({"scope":"conditional_unmodified_category_invocation","formula":"minimum_drops + sum(U24_i < effective_chance, i=1..maximum_drops-minimum_drops)",
        "optional_trials":trials,"authored_chance":category.chance,"uniform_u24_trial_probability":p,
        "expected_count_before_empty_pool_checks":f64::from(category.minimum_drops)+f64::from(trials)*p,
        "runtime_effective_chance":null,"avatar_modifier_stat":if category.index==0 {Value::Null} else {json!(61)},
        "category_modifier_reference":category.modifier_reference,
        "assumptions":["no runtime chance modification","uniform U24 marginal"],
        "warning":"minimum count does not create an item from an empty or filtered pool; category counts are not pickup stack amounts"})
}

fn file_key(resource: &str) -> String {
    format!("{}.bin", resource.trim_start_matches('/').replace('/', "_"))
}

fn index_references(value: &Value, at: &str, identities: &mut BTreeMap<String, BTreeSet<String>>) {
    match value {
        Value::String(path) if path.starts_with("/SF/") || path.starts_with("/Lotus/") => {
            identities
                .entry(path.clone())
                .or_default()
                .insert(at.into());
        }
        Value::Object(map) => {
            for (key, value) in map {
                index_references(value, &format!("{at}{}", pointer(&[key])), identities);
            }
        }
        Value::Array(array) => {
            for (index, value) in array.iter().enumerate() {
                index_references(value, &format!("{at}/{index}"), identities);
            }
        }
        _ => (),
    }
}

pub fn run(mut args: Args) -> Result<()> {
    let (packages, packages_hash) = read_json(&args.packages)?;
    if !packages.is_object() {
        bail!("packages must be a resource-path object");
    }
    let (languages, languages_input) = match &args.languages {
        Some(path) => {
            let (value, hash) = read_json(path)?;
            (value, json!({"file":path,"sha256":hash}))
        }
        None => (Value::Null, Value::Null),
    };
    let exe = args
        .exe
        .take()
        .or_else(soulframe_droptables::find_rvas::default_game_exe);
    let exe_digest = exe
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| digest(&b));
    let matched_native = exe_digest.as_deref() == Some(NATIVE_SHA256);
    args.levels.sort_unstable();
    args.levels.dedup();
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut identities = BTreeMap::<String, BTreeSet<String>>::new();
    for (path, record) in packages.as_object().unwrap() {
        identities
            .entry(path.clone())
            .or_default()
            .insert(format!("{} (resource key)", pointer(&[path])));
        if let Some(attributes) = record.get("attributes") {
            index_references(attributes, &pointer(&[path, "attributes"]), &mut identities);
        }
    }
    for path in identities.keys() {
        names.entry(file_key(path)).or_default().push(path.clone());
    }
    let mut files = std::fs::read_dir(&args.decrypted_dir)
        .with_context(|| format!("reading {}", args.decrypted_dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|p| p.extension().is_some_and(|e| e == "bin"));
    files.sort();
    if files.is_empty() {
        bail!(
            "no decrypted .bin inputs in {}",
            args.decrypted_dir.display()
        );
    }
    let mut tables = Map::new();
    let mut unbound_tables = Map::new();
    let mut decoded = BTreeMap::<String, DecodedDropTable>::new();
    let mut unsupported = Vec::new();
    for path in files {
        let bytes = std::fs::read(&path)?;
        let name = path.file_name().unwrap().to_string_lossy();
        let candidates = names.get(name.as_ref());
        let resource = match candidates {
            Some(v) if v.len() == 1 => Some(&v[0]),
            _ => None,
        };
        match decode_categories(&bytes) {
            Ok(table) => {
                let mut value = serde_json::to_value(&table)?;
                value["source"] = json!({"file":path,"sha256":digest(&bytes)});
                value["resource"] = json!(resource);
                value["resource_identity"] = json!({"status":if resource.is_some(){"authored_package_identity"}else{"unresolved_resource_identity"},
                    "package_pointers":resource.and_then(|r|identities.get(r)),"candidates":candidates});
                value["name"] = json!(resource.map(|r|source_name(&packages,&languages,r)).unwrap_or(name.to_string()));
                value["quantity_models"] = if matched_native { json!(table.categories.iter().map(quantity_model).collect::<Vec<_>>()) } else { Value::Null };
                value["selector_scenarios"] = selector_scenarios(&table,&packages,&args.levels,matched_native)?;
                value["per_enemy_probability"] = Value::Null;
                value["per_run_probability"] = Value::Null;
                if let Some(resource)=resource {
                    tables.insert(resource.clone(), value);
                    decoded.insert(resource.clone(),table);
                } else { unbound_tables.insert(name.to_string(),value); }
            }
            Err(error) => unsupported.push(json!({"resource":resource,"file":path,"sha256":digest(&bytes),"status":"unsupported_serialization","reason":error.to_string()})),
        }
    }
    if tables.is_empty() {
        bail!("no supported category tables decoded; refusing to write empty successful exports");
    }
    let bindings = source_bindings(&packages, &languages);
    let mut report = json!({"schema_version":1,"kind":"level_aware_drop_table_evidence",
        "generated_at":chrono::Utc::now().to_rfc3339(),
        "input_context":if args.input_context.is_empty(){"existing_snapshot"}else{&args.input_context},
        "inputs":{"packages":{"file":args.packages,"sha256":packages_hash},"languages":languages_input,"decrypted_dir":args.decrypted_dir},
        "native_evidence":{"executable":exe,"sha256":exe_digest,"expected_sha256":NATIVE_SHA256,"matched":matched_native,
            "build":"2026.09.24.09.46","quantity_rva":"0xB41800","collector_rva":"0xFFF90","selector_rva":"0x459500"},
        "selector_rules":{"evidence_build_sha256":NATIVE_SHA256,
            "runtime_rarity_source":"resolved resource via 0x1BAF8A0, not the serialized trailing rarity field",
            "base_rarity_weights_f32":[0.755,0.22,0.02,0.005],
            "formula":["p=max(base[rarity]-(item_bias+interpolated_bias), FLT_MIN)","t=min(interpolated_flip,1)",
                "w=max((p+t*((1-p)-p))/accepted_count_for_rarity, FLT_MIN)","ordered f32 sum, reciprocal, and multiply normalization"],
            "scan":"uniform U24; promote each f32 weight to f64; inclusive comparison and sequential subtraction; last-entry fallback",
            "repeated_selection":"0x6F1110 rebuilds the pool for each count iteration; stable inputs permit replacement, but LCG draw independence is a separate model"},
        "level_semantics":{"input":"unsigned native generation input; caller-dependent, not automatically player/enemy/foundry level",
            "comparison":"convert u32 to f32; both group endpoints inclusive; append every eligible overlapping group",
            "sample_levels":args.levels,"sampled_scenarios_are_not_all_possible_levels":true},
        "summary":{"decoded_tables":tables.len()+unbound_tables.len(),"resource_resolved_tables":tables.len(),"unbound_decoded_tables":unbound_tables.len(),"unsupported_inputs":unsupported.len(),"source_bindings":bindings.len()},
        "tables":tables,"unbound_tables":unbound_tables,"unsupported_inputs":unsupported,"source_bindings":bindings,
        "aggregation":{"status":"unresolved_without_observed_invocations","per_enemy_probabilities":null,"per_run_probabilities":null,
            "required_evidence":["actual native table calls and owner/run identity","selected table after dynamic swaps","generation level","runtime resource/type acceptance and rarity","interpolation inputs","effective category chance","RNG dependence model"]},
        "limitations":["raw category schema is strict; unsupported formats and undecoded tails remain explicit",
            "nested item/table references and generic randomized weapon identities are not expanded",
            "source bindings establish authored links, not spawn frequency or invocation counts",
            "script branch landmarks describe the September 25 script snapshot, not a bytecode check of new installations",
            "native formulas are enabled only for the recorded executable hash; package and raw hashes identify each input independently"]});
    if let Some(path) = &args.invocations {
        report["aggregation"] =
            observed_invocations(path, &decoded, matched_native, &packages_hash)?;
    }
    let portable_tables = crate::loot_data::droptables(&report, &packages, &languages);
    std::fs::create_dir_all(&args.output)?;
    let destination = args.output.join("droptables_resolved.json");
    let mut bytes = serde_json::to_vec_pretty(&portable_tables)?;
    bytes.push(b'\n');
    std::fs::write(&destination, bytes)
        .with_context(|| format!("writing {}", destination.display()))?;
    println!("  [resolved loot] {}", destination.display());
    Ok(())
}

// Kept separate so native selector assumptions cannot leak into source cardinality.
fn selector_scenarios(
    table: &DecodedDropTable,
    packages: &Value,
    levels: &[u32],
    matched: bool,
) -> Result<Value> {
    if !matched {
        return Ok(json!({"status":"unresolved_native_build_mismatch"}));
    }
    let mut scenarios = Vec::new();
    for &level in levels {
        let mut categories = Vec::new();
        for category in &table.categories {
            let authored =
                soulframe_droptables::resolved_selector::authored_baseline_inputs(category, level);
            let package =
                crate::loot_resource_inputs::package_baseline_inputs(packages, category, level);
            categories.push(json!({"category_index":category.index,
                "package_resource_rarity":soulframe_droptables::resolved_selector::resolve_category(category,&package)?,
                "serialized_rarity_scenario":soulframe_droptables::resolved_selector::resolve_category(category,&authored)?}));
        }
        scenarios.push(json!({"generation_level":level,"categories":categories}));
    }
    Ok(json!({"status":"conditional_input_scenarios","scenarios":scenarios}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationTrace {
    schema_version: u32,
    native_sha256: String,
    packages_sha256: String,
    evidence_source: String,
    capture_complete: bool,
    independent_uniform_draws_model: bool,
    calls: Vec<ObservedCall>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedCall {
    invocation_id: String,
    run_id: String,
    enemy_id: Option<String>,
    selected_table: String,
    table_sha256: String,
    selection_evidence: String,
    generation_level: u32,
    categories: Vec<ObservedCategory>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedCategory {
    category_index: usize,
    effective_chance: f32,
    context_evidence: String,
    /// Repeated selector draws must use the same pool for the formula below.
    selection_with_replacement: bool,
    selector: soulframe_droptables::resolved_selector::SelectorInputs,
}

fn no_item_probability(minimum: u32, maximum: u32, trial_p: f64, item_p: f64) -> f64 {
    (1.0 - item_p).powf(f64::from(minimum))
        * (1.0 - trial_p * item_p).powf(f64::from(maximum - minimum))
}

fn combine_no_item(target: &mut BTreeMap<String, f64>, incoming: &BTreeMap<String, f64>) {
    for (item, no_item) in incoming {
        *target.entry(item.clone()).or_insert(1.0) *= no_item;
    }
}

fn probabilities_from_no_item(no_item: &BTreeMap<String, f64>) -> Value {
    json!(no_item
        .iter()
        .map(|(item, p)| json!({"item":item,"probability_at_least_one":(1.0-p).clamp(0.0,1.0)}))
        .collect::<Vec<_>>())
}

fn observed_invocations(
    path: &Path,
    tables: &BTreeMap<String, DecodedDropTable>,
    matched: bool,
    packages_hash: &str,
) -> Result<Value> {
    let (value, hash) = read_json(path)?;
    let trace: InvocationTrace =
        serde_json::from_value(value).context("invalid invocation trace")?;
    ensure!(
        trace.schema_version == 1,
        "unsupported invocation trace version"
    );
    ensure!(
        matched && trace.native_sha256 == NATIVE_SHA256,
        "invocation trace/native executable build mismatch"
    );
    ensure!(
        trace.packages_sha256 == packages_hash,
        "invocation trace/package input hash mismatch"
    );
    ensure!(
        !trace.evidence_source.trim().is_empty(),
        "invocation trace requires evidence_source"
    );
    ensure!(
        !trace.calls.is_empty(),
        "invocation trace contains no calls"
    );
    let mut identifiers = BTreeSet::new();
    let mut call_rows = Vec::new();
    let mut runs: BTreeMap<String, (usize, BTreeMap<String, f64>)> = BTreeMap::new();
    let mut enemies: BTreeMap<(String, String), (usize, BTreeMap<String, f64>)> = BTreeMap::new();
    let mut all_resolved = true;
    for call in &trace.calls {
        ensure!(
            !call.invocation_id.trim().is_empty() && !call.run_id.trim().is_empty(),
            "observed calls need invocation_id and run_id"
        );
        ensure!(
            identifiers.insert(&call.invocation_id),
            "duplicate invocation_id {}",
            call.invocation_id
        );
        ensure!(
            !call.selection_evidence.trim().is_empty(),
            "call {} lacks table selection evidence",
            call.invocation_id
        );
        let table = tables.get(&call.selected_table).with_context(|| {
            format!("unknown/unsupported selected table {}", call.selected_table)
        })?;
        ensure!(
            call.table_sha256 == table.source_sha256,
            "call {} table hash mismatch",
            call.invocation_id
        );
        ensure!(
            call.categories.len() == table.categories.len(),
            "call {} must provide all eight category contexts",
            call.invocation_id
        );
        let mut seen_categories = BTreeSet::new();
        let mut category_rows = Vec::new();
        let mut no_item = BTreeMap::<String, f64>::new();
        let mut reasons = Vec::<String>::new();
        for context in &call.categories {
            let category = table
                .categories
                .get(context.category_index)
                .context("observed category index outside table")?;
            ensure!(
                seen_categories.insert(context.category_index),
                "duplicate category context"
            );
            ensure!(
                context.effective_chance.is_finite()
                    && (0.0..=1.0).contains(&context.effective_chance),
                "effective_chance must be finite and clamped to [0,1]"
            );
            ensure!(
                !context.context_evidence.trim().is_empty(),
                "missing category context evidence"
            );
            ensure!(
                context.selector.generation_level == call.generation_level,
                "category level differs from its observed invocation"
            );
            ensure!(
                context.selector.assumptions.is_empty(),
                "observed category context cannot reuse an assumed baseline"
            );
            ensure!(
                context
                    .selector
                    .entry_evidence
                    .values()
                    .all(|e| !e.source.trim().is_empty()),
                "entry evidence requires provenance"
            );
            let selected = soulframe_droptables::resolved_selector::resolve_category(
                category,
                &context.selector,
            )?;
            let p = quantity_trial_probability(context.effective_chance);
            if category.maximum_drops > 1 && !context.selection_with_replacement {
                reasons.push(format!(
                    "category {} repeated selection replacement policy is unverified",
                    category.index
                ));
            }
            if !selected.unresolved.is_empty() {
                reasons.extend(selected.unresolved.clone());
            }
            let mut per_item = BTreeMap::<String, f64>::new();
            for entry in &selected.entries {
                if let Some(probability) = entry.conditional_probability {
                    *per_item.entry(entry.path.clone()).or_default() += probability;
                }
            }
            for (item, q) in &per_item {
                ensure!(
                    *q <= 1.0 + 1e-12,
                    "combined duplicate-entry probability exceeds one"
                );
                *no_item.entry(item.clone()).or_insert(1.0) *= no_item_probability(
                    category.minimum_drops,
                    category.maximum_drops,
                    p,
                    q.min(1.0),
                );
            }
            category_rows.push(json!({"category_index":category.index,"effective_chance":context.effective_chance,
                "uniform_u24_trial_probability":p,"minimum_drops":category.minimum_drops,"maximum_drops":category.maximum_drops,
                "context_evidence":context.context_evidence,"selection_with_replacement":context.selection_with_replacement,"selector":selected}));
        }
        let resolved = reasons.is_empty();
        all_resolved &= resolved;
        let run = runs.entry(call.run_id.clone()).or_default();
        run.0 += 1;
        if resolved {
            combine_no_item(&mut run.1, &no_item);
        }
        if let Some(enemy) = &call.enemy_id {
            ensure!(
                !enemy.trim().is_empty(),
                "empty enemy_id; use null for a non-enemy call"
            );
            let enemy = enemies
                .entry((call.run_id.clone(), enemy.clone()))
                .or_default();
            enemy.0 += 1;
            if resolved {
                combine_no_item(&mut enemy.1, &no_item);
            }
        }
        call_rows.push(json!({"invocation_id":call.invocation_id,"run_id":call.run_id,"enemy_id":call.enemy_id,
            "selected_table":call.selected_table,"table_sha256":call.table_sha256,"selection_evidence":call.selection_evidence,
            "generation_level":call.generation_level,"categories":category_rows,"unresolved":reasons,
            "conditional_item_probabilities":if resolved && trace.independent_uniform_draws_model {probabilities_from_no_item(&no_item)}else{Value::Null}}));
    }
    let combine = all_resolved && trace.capture_complete && trace.independent_uniform_draws_model;
    Ok(
        json!({"status":if combine{"conditional_observed_context_model"}else{"observed_counts_with_unresolved_aggregation"},
        "input":{"file":path,"sha256":hash,"evidence_source":trace.evidence_source},
        "evidence_origin":"supplied observations; this command does not attach to or record the game",
        "observed_native_invocations":trace.calls.len(),"capture_complete":trace.capture_complete,
        "incomplete_counts_are_lower_bounds":!trace.capture_complete,
        "independent_uniform_draws_model":trace.independent_uniform_draws_model,
        "scope":"at least one direct entry across these supplied invocation contexts; not a forecast of arbitrary future enemies or runs",
        "formula":"P(no item in category)=(1-q)^minimum * (1-p*q)^(maximum-minimum); combine no-item probabilities across supplied calls only under the declared independent model",
        "calls":call_rows,
        "enemies":enemies.iter().map(|((run,enemy),(count,odds))|json!({"run_id":run,"enemy_id":enemy,"observed_native_invocations":count,
            "conditional_item_probabilities":if combine{probabilities_from_no_item(odds)}else{Value::Null}})).collect::<Vec<_>>(),
        "runs":runs.iter().map(|(run,(count,odds))|json!({"run_id":run,"observed_native_invocations":count,
            "conditional_item_probabilities":if combine{probabilities_from_no_item(odds)}else{Value::Null}})).collect::<Vec<_>>() }),
    )
}

#[cfg(test)]
#[path = "loot_resolved_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_nested_fields_preserve_null_and_cycles() {
        let p = json!({"child":{"parent":"parent","attributes":{"Script":{"Table":null}}},
            "parent":{"parent":"child","attributes":{"Script":{"Table":"table","Function":"Run"}}}});
        assert!(field(&p, "child", &["Script", "Table"]).is_null());
        assert_eq!(field(&p, "child", &["Script", "Function"]), "Run");
        assert!(field(&p, "child", &["Missing"]).is_null());
    }
    #[test]
    fn rider_predeath_link_requires_its_handler_and_ring_switch() {
        let mut packages = json!({"/SF/RiderBase":{"attributes":{"DropTable":null,"OnPreDeathScript":{
            "Script":"/SF/Scripts/Npc/MendicantRider.lua","Function":"OnRiderDeath","_shouldSpawnMushroomRing":0}}},
            "/SF/RiderBoss":{"parent":"/SF/RiderBase","attributes":{"OnPreDeathScript":{"_shouldSpawnMushroomRing":1,"_mushroomPickup":"/SF/RiderChest"}}}});
        let link = mushroom_enemy_binding(&packages, "/SF/RiderBoss", "/SF/RiderChest").unwrap();
        assert_eq!(link["trigger"], "pre_death");
        packages["/SF/RiderBoss"]["attributes"]["OnPreDeathScript"]["_shouldSpawnMushroomRing"] =
            json!(0);
        assert!(mushroom_enemy_binding(&packages, "/SF/RiderBoss", "/SF/RiderChest").is_none());
        packages["/SF/RiderBoss"]["attributes"]["OnPreDeathScript"]["_shouldSpawnMushroomRing"] =
            json!(1);
        packages["/SF/RiderBoss"]["attributes"]["OnPreDeathScript"]["Function"] =
            json!("Unrelated");
        assert!(mushroom_enemy_binding(&packages, "/SF/RiderBoss", "/SF/RiderChest").is_none());
    }
    #[test]
    fn death_hook_can_call_the_ring_export_directly() {
        let packages = json!({"/SF/King":{"attributes":{"OnDeathScript":{
            "Script":"/SF/Scripts/Npc/SubbossRednightResourceSpawn.lua","Function":"SpawnShroomRing","_mushroomPickup":"/SF/KingChest"}}}});
        let link = mushroom_enemy_binding(&packages, "/SF/King", "/SF/KingChest").unwrap();
        assert_eq!(link["trigger"], "death");
    }
    #[test]
    fn dog_purification_route_resolves_relative_entity_at_declaring_resource() {
        let script = "/SF/Types/Enemies/Sinecure/OdeAttackDog/DogScripts.lua";
        let mut packages = json!({"/SF/Dog/Base":{"attributes":{"OnPreDeathScript":{
            "Script":script,"Function":"OnCorruptedPreDeath","_dogPurifySseType":"Purification"}}},
            "/SF/AnotherDirectory/Boss":{"parent":"/SF/Dog/Base"},
            "/SF/Dog/Purification":{"attributes":{"Script":{"Script":script,"Function":"OnPreDeathSteal","_shouldSpawnMushroomRing":1,"_mushroomPickup":"/SF/DogChest"}}}});
        let link =
            mushroom_enemy_binding(&packages, "/SF/AnotherDirectory/Boss", "/SF/DogChest").unwrap();
        assert_eq!(link["trigger"], "purification");
        assert_eq!(link["entity"], "/SF/Dog/Purification");
        packages["/SF/Dog/Purification"]["attributes"]["Script"]["_shouldSpawnMushroomRing"] =
            json!(0);
        assert!(
            mushroom_enemy_binding(&packages, "/SF/AnotherDirectory/Boss", "/SF/DogChest")
                .is_none()
        );
    }
    #[test]
    fn null_direct_table_blocks_parent_table_and_disabled_chance_survives() {
        let packages = json!({"/SF/Base":{"attributes":{"DropTable":"/SF/Table","DropChance":1}},
            "/SF/Rider":{"parent":"/SF/Base","attributes":{"DropTable":null}},
            "/SF/OtherBoss":{"parent":"/SF/Base","attributes":{"DropChance":0}}});
        let bindings = source_bindings(&packages, &Value::Null);
        assert!(!bindings.iter().any(|b| b["source"] == "/SF/Rider"));
        let disabled = bindings
            .iter()
            .find(|b| b["source"] == "/SF/OtherBoss")
            .unwrap();
        assert_eq!(disabled["authored_drop_chance"], 0);
    }
    #[test]
    fn category_count_grid_uses_strict_less_than() {
        assert_eq!(quantity_trial_probability(0.0), 0.0);
        assert_eq!(quantity_trial_probability(1.0), 1.0);
        assert_eq!(quantity_trial_probability(0.5), 0.5);
        assert_eq!(
            quantity_trial_probability(f32::MIN_POSITIVE),
            1.0 / (1u32 << 24) as f64
        );
    }
    #[test]
    fn guaranteed_minimum_and_optional_trials_are_distinct() {
        assert_eq!(no_item_probability(1, 1, 0.0, 0.25), 0.75);
        assert_eq!(no_item_probability(0, 1, 0.8, 0.25), 0.8);
        assert!(
            (no_item_probability(2, 4, 0.5, 0.25) - 0.75_f64.powi(2) * 0.875_f64.powi(2)).abs()
                < 1e-15
        );
    }
    #[test]
    fn null_hard_chest_table_does_not_fall_back() {
        let p = json!({"/SF/Chest":{"attributes":{"PickUpScript":{"Script":LOOT_CHEST,"Function":"MushroomChest","_dropTable":"/SF/Table","_hardmodeDroptable":null}}}});
        let bindings = source_bindings(&p, &Value::Null);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["hard_mode_null_behavior"], "no_normal_fallback");
        assert!(bindings[0]["hard_mode_table"]["value"].is_null());
    }
    #[test]
    fn empty_hard_zone_override_keeps_region_base() {
        let p = json!({"/SF/Enemy":{"attributes":{"ScriptInstance":{"Script":ZONE_OVERRIDE,"Function":"OnSpawn","_regionLevelInfos":["/SF/Region"],"_dropTableOverrides":["/SF/Table"],"_hardModeDropTableOverrides":{}}}}});
        let b = source_bindings(&p, &Value::Null);
        assert_eq!(b[0]["hard_mode_table"], "/SF/Table");
        assert_eq!(b[0]["has_hard_mode_replacement"], false);
    }
}
