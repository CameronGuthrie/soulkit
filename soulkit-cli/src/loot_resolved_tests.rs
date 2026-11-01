//! Synthetic end-to-end invocation-model fixtures; no observations of the game.

use super::*;
use soulframe_droptables::resolved_raw::{DropCategory, DropEntry, LevelGroup};
use soulframe_droptables::resolved_selector::{
    authored_baseline_inputs, resolve_category, SelectorInputs,
};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const TABLE: &str = "/SF/Test/ObservedTable";
const TABLE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PACKAGES_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn export_table_fixture() -> Vec<u8> {
    // One supported counted category with one item; seven empty categories.
    let mut bytes = vec![0; 12];
    bytes[8] = 14;
    bytes.extend(0_u32.to_le_bytes());
    for index in 0..8 {
        bytes.extend([0, 0, 0]);
        bytes.extend(1_f32.to_le_bytes());
        bytes.extend(0_u32.to_le_bytes()); // minimum drops
        bytes.extend(1_u32.to_le_bytes()); // maximum drops
        bytes.extend(0_u32.to_le_bytes()); // quality tiers
        bytes.extend(0_u32.to_le_bytes()); // modifier
        bytes.extend(u32::from(index == 0).to_le_bytes()); // groups
        if index == 0 {
            bytes.push(0); // no interpolation
            bytes.extend(1_f32.to_le_bytes());
            bytes.extend(100_f32.to_le_bytes());
            bytes.extend(1_u32.to_le_bytes()); // items
            bytes.extend(0_u32.to_le_bytes()); // item reference kind
            let path = b"/SF/Test/Item";
            bytes.extend((path.len() as u32).to_le_bytes());
            bytes.extend(path);
            bytes.extend(0_f32.to_le_bytes()); // bias
            bytes.extend(0_u32.to_le_bytes()); // rarity
            bytes.extend(1_f32.to_le_bytes()); // authored weight
        }
    }
    let tail = b"/Lotus/Types/Items/DefaultStrippedItem";
    bytes.extend((tail.len() as u32).to_le_bytes());
    bytes.extend(tail);
    bytes
}

#[test]
fn public_export_writes_only_drop_tables_and_preserves_existing_chance_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/public-loot-export-tests")
        .join(format!(
            "{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
    let decrypted = root.join("decrypted");
    let output = root.join("output");
    std::fs::create_dir_all(&decrypted).unwrap();
    let packages = root.join("packages.json");
    std::fs::write(
        &packages,
        serde_json::to_vec(&json!({"/SF/Test/Table": {"attributes": {}}})).unwrap(),
    )
    .unwrap();
    let bytes = export_table_fixture();
    assert_eq!(decode_categories(&bytes).unwrap().entry_count(), 1);
    std::fs::write(decrypted.join("SF_Test_Table.bin"), bytes).unwrap();
    let request = || Args {
        packages: packages.clone(),
        decrypted_dir: decrypted.clone(),
        output: output.clone(),
        exe: Some(root.join("no-client.exe")),
        levels: vec![1],
        languages: None,
        invocations: None,
        input_context: "public_export_test".into(),
    };

    run(request()).unwrap();
    let files: Vec<_> = std::fs::read_dir(&output)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        files,
        [std::ffi::OsString::from("droptables_resolved.json")]
    );
    let exported: Value =
        serde_json::from_slice(&std::fs::read(output.join("droptables_resolved.json")).unwrap())
            .unwrap();
    assert_eq!(exported["tables"].as_array().unwrap().len(), 1);

    // Running the public exporter over an existing output folder must not
    // recreate, modify or delete chance reports produced by another build.
    for name in ["moonsteel_chance.json", "craftwork_tier_chance.json"] {
        std::fs::write(output.join(name), b"existing report").unwrap();
    }
    run(request()).unwrap();
    for name in ["moonsteel_chance.json", "craftwork_tier_chance.json"] {
        assert_eq!(
            std::fs::read(output.join(name)).unwrap(),
            b"existing report"
        );
    }
}

struct TraceFile(PathBuf);

impl TraceFile {
    fn new(value: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "soulkit-observed-invocation-test-{}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
        Self(path)
    }
}

impl Drop for TraceFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn fixture() -> (BTreeMap<String, DecodedDropTable>, Value) {
    let mut categories: Vec<_> = (0..8)
        .map(|index| DropCategory {
            index,
            chance: 0.0,
            ..Default::default()
        })
        .collect();
    categories[7] = DropCategory {
        index: 7,
        chance: 0.8, // Different from the observed effective 0.5 on purpose.
        minimum_drops: 1,
        maximum_drops: 2,
        quality_indices: vec![2, 3, 4, 5, 6],
        level_groups: vec![LevelGroup {
            index: 0,
            minimum_level: 0.0,
            maximum_level: 100.0,
            items: ["/SF/Test/A", "/SF/Test/A", "/SF/Test/B"]
                .iter()
                .enumerate()
                .map(|(index, path)| DropEntry {
                    index,
                    file_offset: 101 + index,
                    end_offset: 102 + index,
                    path: (*path).into(),
                    rarity_index: 0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let contexts: Vec<_> = categories
        .iter()
        .map(|category| {
            let mut selector = authored_baseline_inputs(category, 35);
            selector.assumptions.clear();
            for evidence in selector.entry_evidence.values_mut() {
                evidence.source =
                    "synthetic fixture: explicit accepted reference and runtime rarity".into();
            }
            json!({
                "category_index": category.index,
                "effective_chance": if category.index == 7 { 0.5 } else { 0.0 },
                "context_evidence": "synthetic fixture: category context supplied explicitly",
                "selection_with_replacement": true,
                "selector": selector,
            })
        })
        .collect();
    let call = |id: &str| {
        json!({
            "invocation_id": id, "run_id": "run-1", "enemy_id": "enemy-1",
            "selected_table": TABLE, "table_sha256": TABLE_HASH,
            "selection_evidence": "synthetic fixture: selected table after mode resolution",
            "generation_level": 35, "categories": contexts,
        })
    };
    let trace = json!({
        "schema_version": 1, "native_sha256": NATIVE_SHA256,
        "packages_sha256": PACKAGES_HASH, "evidence_source": "synthetic unit fixture, not a game capture",
        "capture_complete": true, "independent_uniform_draws_model": true,
        "calls": [call("call-1"), call("call-2")],
    });
    let table = DecodedDropTable {
        source_sha256: TABLE_HASH.into(),
        categories,
        ..Default::default()
    };
    (BTreeMap::from([(TABLE.into(), table)]), trace)
}

fn run_trace(tables: &BTreeMap<String, DecodedDropTable>, trace: &Value) -> Result<Value> {
    let file = TraceFile::new(trace);
    observed_invocations(&file.0, tables, true, PACKAGES_HASH)
}

fn item_probability(rows: &Value, path: &str) -> f64 {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["item"] == path)
        .unwrap()["probability_at_least_one"]
        .as_f64()
        .unwrap()
}

#[test]
fn two_calls_combine_duplicate_paths_and_observed_effective_count_chance() {
    let (tables, trace) = fixture();
    let output = run_trace(&tables, &trace).unwrap();
    assert_eq!(output["status"], "conditional_observed_context_model");
    assert_eq!(output["observed_native_invocations"], 2);
    assert_eq!(output["runs"][0]["observed_native_invocations"], 2);
    assert_eq!(output["enemies"][0]["observed_native_invocations"], 2);
    assert_eq!(
        output["calls"][0]["conditional_item_probabilities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // The selector has independent focused tests for exact U24 masses. Here use
    // its duplicate-entry mass to verify the distinct quantity/cardinality layer.
    let inputs: SelectorInputs =
        serde_json::from_value(trace["calls"][0]["categories"][7]["selector"].clone()).unwrap();
    let selected = resolve_category(&tables[TABLE].categories[7], &inputs).unwrap();
    let q = selected
        .entries
        .iter()
        .filter(|entry| entry.path == "/SF/Test/A")
        .map(|entry| entry.conditional_probability.unwrap())
        .sum::<f64>();
    assert!((0.66..0.67).contains(&q));
    let one_call_none = (1.0 - q) * (1.0 - 0.5 * q);
    let one_call = item_probability(
        &output["calls"][0]["conditional_item_probabilities"],
        "/SF/Test/A",
    );
    let two_calls = item_probability(
        &output["runs"][0]["conditional_item_probabilities"],
        "/SF/Test/A",
    );
    assert!((one_call - (1.0 - one_call_none)).abs() < 1e-15);
    assert!((two_calls - (1.0 - one_call_none * one_call_none)).abs() < 1e-15);
    assert_eq!(
        two_calls,
        item_probability(
            &output["enemies"][0]["conditional_item_probabilities"],
            "/SF/Test/A"
        )
    );
    assert_eq!(
        output["calls"][0]["categories"][7]["uniform_u24_trial_probability"],
        0.5
    );
}

#[test]
fn native_package_and_table_hash_mismatches_are_rejected() {
    let (tables, original) = fixture();
    for pointer in [
        "/native_sha256",
        "/packages_sha256",
        "/calls/0/table_sha256",
    ] {
        let mut trace = original.clone();
        *trace.pointer_mut(pointer).unwrap() = json!("mismatch");
        assert!(run_trace(&tables, &trace).is_err(), "accepted {pointer}");
    }
    let file = TraceFile::new(&original);
    assert!(observed_invocations(&file.0, &tables, false, PACKAGES_HASH).is_err());
}

#[test]
fn duplicate_invocation_ids_and_duplicate_or_missing_categories_are_rejected() {
    let (tables, original) = fixture();
    let mut trace = original.clone();
    trace["calls"][1]["invocation_id"] = json!("call-1");
    assert!(run_trace(&tables, &trace)
        .unwrap_err()
        .to_string()
        .contains("duplicate invocation_id"));
    let mut trace = original.clone();
    trace["calls"][0]["categories"][0]["category_index"] = json!(7);
    assert!(run_trace(&tables, &trace)
        .unwrap_err()
        .to_string()
        .contains("duplicate category"));
    let mut trace = original;
    trace["calls"][0]["categories"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(run_trace(&tables, &trace)
        .unwrap_err()
        .to_string()
        .contains("all eight category"));
}

#[test]
fn partial_capture_keeps_counts_and_call_models_but_withholds_enemy_and_run_models() {
    let (tables, mut trace) = fixture();
    trace["capture_complete"] = json!(false);
    let output = run_trace(&tables, &trace).unwrap();
    assert_eq!(output["observed_native_invocations"], 2);
    assert_eq!(output["incomplete_counts_are_lower_bounds"], true);
    assert!(output["calls"][0]["conditional_item_probabilities"].is_array());
    assert!(output["runs"][0]["conditional_item_probabilities"].is_null());
    assert!(output["enemies"][0]["conditional_item_probabilities"].is_null());
}

#[test]
fn absence_of_independence_model_withholds_every_combined_item_probability() {
    let (tables, mut trace) = fixture();
    trace["independent_uniform_draws_model"] = json!(false);
    let output = run_trace(&tables, &trace).unwrap();
    assert!(output["calls"][0]["conditional_item_probabilities"].is_null());
    assert!(output["runs"][0]["conditional_item_probabilities"].is_null());
    assert!(output["enemies"][0]["conditional_item_probabilities"].is_null());
    assert_eq!(output["observed_native_invocations"], 2);
}

#[test]
fn unknown_entry_evidence_blocks_its_call_and_whole_capture_aggregation() {
    let (tables, mut trace) = fixture();
    trace["calls"][0]["categories"][7]["selector"]["entry_evidence"]
        .as_object_mut()
        .unwrap()
        .remove("101");
    let output = run_trace(&tables, &trace).unwrap();
    assert!(output["calls"][0]["conditional_item_probabilities"].is_null());
    assert!(!output["calls"][0]["unresolved"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(output["calls"][1]["conditional_item_probabilities"].is_array());
    assert!(output["runs"][0]["conditional_item_probabilities"].is_null());
    assert!(output["enemies"][0]["conditional_item_probabilities"].is_null());
}

#[test]
fn unverified_replacement_blocks_repeated_selection_and_assumed_baseline_is_rejected() {
    let (tables, original) = fixture();
    let mut trace = original.clone();
    trace["calls"][0]["categories"][7]["selection_with_replacement"] = json!(false);
    let output = run_trace(&tables, &trace).unwrap();
    assert!(output["calls"][0]["conditional_item_probabilities"].is_null());
    assert!(output["runs"][0]["conditional_item_probabilities"].is_null());
    let mut trace = original;
    trace["calls"][0]["categories"][7]["selector"]["assumptions"] =
        json!(["assumed resource rarity"]);
    assert!(run_trace(&tables, &trace)
        .unwrap_err()
        .to_string()
        .contains("assumed baseline"));
}
