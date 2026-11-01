//! Current-build SFDropTable item selection, conditional on supplied runtime facts.
//!
//! Evidence: build 2026.09.24.09.46; group collector RVA 0x000fff90,
//! bias/rarity normalization 0x00459500, U24 selection 0x006f1315.
//! This does not determine table invocation counts, upgrades, resource eligibility,
//! or resource rarity. The authored baseline explicitly assumes those inputs.

use crate::resolved_raw::{DropCategory, LevelGroup};
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const NATIVE_BUILD_SHA256: &str =
    "e90536169d0ee4ce9222b0a90d7e9bf071d306f5f53d9be7a74cf63ab4a9d00b";
pub const RARITY_BASE_WEIGHTS: [f32; 4] = [0.755, 0.22, 0.02, 0.005];
pub const U24_DENOMINATOR: u32 = 1 << 24;
/// Tool-only resource limit; below the point at which f32 rarity counts lose units.
pub const MAX_ACCEPTED_ENTRIES: usize = 4096;

/// SFDropTable +0x314 + category*24, used when a group has no own interpolation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CategoryInterpolation {
    pub minimum_level: f32,
    pub maximum_level: f32,
    pub bias_start: f32,
    pub bias_end: f32,
    pub flip_start: f32,
    pub flip_end: f32,
}

impl Default for CategoryInterpolation {
    fn default() -> Self {
        // Constructor 0x01769cc3 onward. Authored/inherited overrides are separate.
        Self {
            minimum_level: 0.0,
            maximum_level: 100.0,
            bias_start: 0.0,
            bias_end: 0.0,
            flip_start: 0.0,
            flip_end: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EntryEvidence {
    /// Includes reference resolution, native exclusion checks and category resource.
    pub accepted: Option<bool>,
    /// Result of native 0x01baf8a0; the serialized rarity is not authoritative.
    pub runtime_rarity: Option<u32>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SelectorInputs {
    pub generation_level: u32,
    /// Runtime global force-all mode must be false for the random selection path.
    #[serde(default)]
    pub random_selection_mode: Option<bool>,
    pub category_interpolation: Option<CategoryInterpolation>,
    /// Entry file offset -> caller-supplied evidence; missing facts remain unknown.
    pub entry_evidence: BTreeMap<usize, EntryEvidence>,
    pub assumptions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SelectedEntry {
    pub group_index: usize,
    pub entry_index: usize,
    pub file_offset: usize,
    pub path: String,
    pub reference_kind: u32,
    pub authored_rarity_index: u32,
    /// Exposed for audit; 0x459500 replaces the serialized weight.
    pub authored_weight: f32,
    pub item_bias: f32,
    pub status: String,
    pub evidence: Option<EntryEvidence>,
    pub interpolated_bias: Option<f32>,
    pub interpolated_flip: Option<f32>,
    pub weight_before_normalization: Option<f32>,
    pub normalized_weight: Option<f32>,
    /// Exact scan mass on a uniform U24 grid for the supplied conditional model.
    pub u24_numerator_count: Option<u32>,
    pub conditional_probability: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResolvedCategory {
    pub native_build_sha256: String,
    pub category_index: usize,
    pub generation_level: u32,
    pub generation_level_f32: f32,
    pub scope: String,
    pub status: String,
    pub assumptions: Vec<String>,
    pub unresolved: Vec<String>,
    pub eligible_group_indices: Vec<usize>,
    pub rarity_counts: [u32; 4],
    pub total_weight_f32: Option<f32>,
    pub entries: Vec<SelectedEntry>,
}

/// Explicit scenario only: the raw file alone does not prove these assumptions.
pub fn authored_baseline_inputs(category: &DropCategory, level: u32) -> SelectorInputs {
    let entry_evidence = category
        .level_groups
        .iter()
        .flat_map(|g| &g.items)
        .map(|e| {
            (
                e.file_offset,
                EntryEvidence {
                    accepted: Some(true),
                    runtime_rarity: Some(e.rarity_index),
                    source:
                        "scenario: all resource filters pass; runtime rarity equals authored rarity"
                            .into(),
                },
            )
        })
        .collect();
    SelectorInputs {
        generation_level: level,
        random_selection_mode: Some(true),
        category_interpolation: Some(CategoryInterpolation::default()),
        entry_evidence,
        assumptions: vec![
            "Conditional item selection after the category collector runs; not per enemy or per run".into(),
            "All level-eligible references pass native resource/exclusion/category-resource checks".into(),
            "Runtime resource rarity equals each serialized rarity_index".into(),
            "Non-overridden groups use native constructor interpolation defaults [0,100,0,0,0,0]".into(),
            "Uniform U24 variate; no claim of independent game seeds or repeated invocations".into(),
            "Runtime force-all-items flag is disabled; authored category all-items flags are checked".into(),
        ],
    }
}

/// Resolve overlapping groups in file order. Any unknown accepted entry prevents
/// normalization of the whole category; it is never silently removed from a pool.
pub fn resolve_category(
    category: &DropCategory,
    inputs: &SelectorInputs,
) -> Result<ResolvedCategory> {
    let level = inputs.generation_level as f32;
    let mut result = ResolvedCategory {
        native_build_sha256: NATIVE_BUILD_SHA256.into(),
        category_index: category.index,
        generation_level: inputs.generation_level,
        generation_level_f32: level,
        scope: "one conditional SFDropTable item selection with supplied acceptance, rarity and interpolation inputs".into(),
        status: "unresolved".into(),
        assumptions: inputs.assumptions.clone(),
        unresolved: Vec::new(),
        eligible_group_indices: Vec::new(),
        rarity_counts: [0; 4],
        total_weight_f32: None,
        entries: Vec::new(),
    };
    if category.prefix_bytes[1] != 0 {
        result.unresolved.push(
            "category all-items flag bypasses the random selector (native 0x006f12a2)".into(),
        );
    }
    if inputs.random_selection_mode != Some(true) {
        result.unresolved.push("random selection mode is not confirmed; native global force-all flag can bypass the selector".into());
    }
    let mut accepted = Vec::new();
    for group in &category.level_groups {
        ensure!(
            group.minimum_level.is_finite() && group.maximum_level.is_finite(),
            "non-finite group level range"
        );
        let eligible = level >= group.minimum_level && level <= group.maximum_level;
        if eligible {
            result.eligible_group_indices.push(group.index);
        }
        let interpolation = if eligible {
            interpolate(group, inputs.category_interpolation.as_ref(), level)?
        } else {
            None
        };
        for entry in &group.items {
            ensure!(
                entry.bias.is_finite(),
                "non-finite entry bias at {}",
                entry.file_offset
            );
            let evidence = inputs.entry_evidence.get(&entry.file_offset).cloned();
            let mut row = SelectedEntry {
                group_index: group.index,
                entry_index: entry.index,
                file_offset: entry.file_offset,
                path: entry.path.clone(),
                reference_kind: entry.reference_kind,
                authored_rarity_index: entry.rarity_index,
                authored_weight: entry.authored_weight,
                item_bias: entry.bias,
                status: "outside_level_range".into(),
                evidence,
                interpolated_bias: interpolation.map(|p| p.0),
                interpolated_flip: interpolation.map(|p| p.1),
                weight_before_normalization: None,
                normalized_weight: None,
                u24_numerator_count: None,
                conditional_probability: None,
            };
            if eligible {
                match row.evidence.as_ref().and_then(|e| e.accepted) {
                    Some(false) => row.status = "excluded_by_supplied_evidence".into(),
                    None => {
                        row.status = "unresolved_acceptance".into();
                        result.unresolved.push(format!(
                            "entry {}: native resource acceptance unknown",
                            entry.file_offset
                        ));
                    }
                    Some(true) => {
                        let rarity = row.evidence.as_ref().and_then(|e| e.runtime_rarity);
                        if let Some(rarity) = rarity {
                            ensure!(
                                rarity < 4,
                                "runtime rarity {} is outside the native four-tier table",
                                rarity
                            );
                            if let Some((bias, flip)) = interpolation {
                                let combined_bias = entry.bias + bias; // 0x1003e6
                                ensure!(combined_bias.is_finite(), "combined bias overflow");
                                row.status = "accepted".into();
                                accepted.push((
                                    result.entries.len(),
                                    rarity as usize,
                                    combined_bias,
                                    flip,
                                ));
                                result.rarity_counts[rarity as usize] += 1;
                            } else {
                                row.status = "unresolved_interpolation".into();
                                result.unresolved.push(format!(
                                    "group {}: category interpolation unavailable",
                                    group.index
                                ));
                            }
                        } else {
                            row.status = "unresolved_rarity".into();
                            result.unresolved.push(format!(
                                "entry {}: native resource rarity unknown",
                                entry.file_offset
                            ));
                        }
                    }
                }
            }
            result.entries.push(row);
        }
    }
    ensure!(
        accepted.len() <= MAX_ACCEPTED_ENTRIES,
        "selector exceeds tool limit of {} accepted entries",
        MAX_ACCEPTED_ENTRIES
    );
    if !result.unresolved.is_empty() {
        return Ok(result);
    }
    if accepted.is_empty() {
        result.status = "empty_eligible_pool".into();
        return Ok(result);
    }
    let mut weights = Vec::with_capacity(accepted.len());
    let mut total = 0.0_f32;
    for &(row, rarity, bias, flip) in &accepted {
        // Preserve separate scalar SSE operations: do not combine with mul_add.
        let p = (RARITY_BASE_WEIGHTS[rarity] - bias).max(f32::MIN_POSITIVE);
        let t = flip.min(1.0); // Native caps the upper bound only.
        let complement = 1.0_f32 - p;
        let slope = complement - p;
        let shifted = t * slope;
        let blended = shifted + p;
        let weight = (blended / result.rarity_counts[rarity] as f32).max(f32::MIN_POSITIVE);
        ensure!(weight.is_finite(), "native weight overflow");
        total += weight; // Native ordered addss, not f64/compensated summation.
        result.entries[row].weight_before_normalization = Some(weight);
        weights.push(weight);
    }
    ensure!(
        total.is_finite() && total > 0.0,
        "invalid native weight sum"
    );
    let reciprocal = 1.0_f32 / total;
    for weight in &mut weights {
        *weight *= reciprocal;
    }
    let counts = u24_selection_counts(&weights)?;
    for ((row, _, _, _), (weight, count)) in
        accepted.into_iter().zip(weights.into_iter().zip(counts))
    {
        let entry = &mut result.entries[row];
        entry.normalized_weight = Some(weight);
        entry.u24_numerator_count = Some(count);
        entry.conditional_probability = Some(f64::from(count) / f64::from(U24_DENOMINATOR));
    }
    result.total_weight_f32 = Some(total);
    result.status = "conditional_selection_resolved".into();
    Ok(result)
}

fn interpolate(
    group: &LevelGroup,
    category: Option<&CategoryInterpolation>,
    level: f32,
) -> Result<Option<(f32, f32)>> {
    let (minimum, maximum, parameters, clamp_level) = if group.interpolated {
        let p = group
            .interpolation_parameters
            .ok_or_else(|| anyhow::anyhow!("group interpolation flag lacks parameters"))?;
        (group.minimum_level, group.maximum_level, p, false)
    } else {
        let Some(c) = category else {
            return Ok(None);
        };
        (
            c.minimum_level,
            c.maximum_level,
            [c.bias_start, c.bias_end, c.flip_start, c.flip_end],
            true,
        )
    };
    ensure!(
        minimum.is_finite() && maximum.is_finite() && parameters.iter().all(|p| p.is_finite()),
        "non-finite interpolation parameters"
    );
    let span = maximum - minimum;
    ensure!(span.is_finite(), "interpolation range overflow");
    // Both native paths use minimum, not zero, for a non-positive span.
    let t = if span > 0.0 {
        let value = if clamp_level {
            level.max(minimum).min(maximum)
        } else {
            level
        };
        (value - minimum) / span
    } else {
        minimum
    };
    let bias = (parameters[1] - parameters[0]) * t + parameters[0];
    let flip = (parameters[3] - parameters[2]) * t + parameters[2];
    ensure!(
        bias.is_finite() && flip.is_finite(),
        "interpolated bias/flip overflow"
    );
    Ok(Some((bias, flip)))
}

/// Native 0x6f1340 scan: promote f32 weights to f64, accept equality,
/// subtract in order, and fall back to the last entry if rounding leaves a tail.
pub fn select_u24(weights: &[f32], numerator: u32) -> Result<usize> {
    ensure!(numerator < U24_DENOMINATOR, "U24 numerator out of range");
    ensure!(!weights.is_empty(), "empty selector weights");
    ensure!(
        weights.iter().all(|w| w.is_finite() && *w >= 0.0),
        "invalid selector weight"
    );
    Ok(select_validated(weights, numerator))
}

fn select_validated(weights: &[f32], numerator: u32) -> usize {
    let mut remaining = f64::from(numerator) / f64::from(U24_DENOMINATOR);
    for (i, &weight) in weights.iter().enumerate() {
        if remaining <= f64::from(weight) {
            return i;
        }
        remaining -= f64::from(weight);
    }
    weights.len() - 1
}

/// Exact uniform-grid mass without iterating all 2^24 draws. The ordered scan is
/// monotone for nonnegative finite weights, so each cumulative boundary is found
/// by binary search. This includes the inclusive zero draw and final fallback.
pub fn u24_selection_counts(weights: &[f32]) -> Result<Vec<u32>> {
    ensure!(
        weights.len() <= MAX_ACCEPTED_ENTRIES,
        "too many selector weights"
    );
    if weights.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        weights.iter().all(|w| w.is_finite() && *w >= 0.0),
        "invalid selector weight"
    );
    let mut previous = 0;
    let mut counts = Vec::with_capacity(weights.len());
    for i in 0..weights.len() - 1 {
        let mut low = previous;
        let mut high = U24_DENOMINATOR;
        while low < high {
            let mid = low + (high - low) / 2;
            if select_validated(weights, mid) <= i {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        counts.push(low - previous);
        previous = low;
    }
    counts.push(U24_DENOMINATOR - previous);
    if counts.iter().map(|&n| u64::from(n)).sum::<u64>() != u64::from(U24_DENOMINATOR) {
        bail!("U24 count invariant failed");
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn category(groups: serde_json::Value) -> DropCategory {
        serde_json::from_value(json!({
            "index": 7, "file_offset": 0, "end_offset": 1000,
            "prefix_bytes": [0,0,0], "reserved_field": 0, "modifier_reference": null,
            "chance_file_offset": 3, "chance": 0.8, "minimum_drops": 0,
            "maximum_drops": 1, "quality_indices": [2,3,4,5,6], "level_groups": groups
        }))
        .unwrap()
    }
    fn group(index: usize, min: f32, max: f32, items: serde_json::Value) -> serde_json::Value {
        json!({"index": index, "file_offset": index*100, "end_offset": index*100+99,
            "interpolated": false, "interpolation_parameters": null,
            "minimum_level": min, "maximum_level": max, "items": items})
    }
    fn entry(offset: usize, rarity: u32, bias: f32) -> serde_json::Value {
        json!({"index": offset, "file_offset": offset, "end_offset": offset+1,
            "reference_kind": 0, "path": format!("/item/{offset}"), "bias": bias,
            "rarity_index": rarity, "authored_weight": 999.0})
    }
    #[test]
    fn overlapping_ranges_include_both_endpoints_in_file_order() {
        let c = category(json!([
            group(0, 0.0, 100.0, json!([entry(1, 0, 0.0)])),
            group(1, 0.0, 15.0, json!([entry(2, 0, 0.0)])),
            group(2, 15.0, 100.0, json!([entry(3, 0, 0.0)]))
        ]));
        for (level, expected) in [(14, vec![0, 1]), (15, vec![0, 1, 2]), (16, vec![0, 2])] {
            let r = resolve_category(&c, &authored_baseline_inputs(&c, level)).unwrap();
            assert_eq!(r.eligible_group_indices, expected);
            assert_eq!(
                r.entries
                    .iter()
                    .filter(|e| e.normalized_weight.is_some())
                    .count(),
                expected.len()
            );
        }
    }
    #[test]
    fn rarity_mass_is_split_by_count_and_authored_weight_is_replaced() {
        let c = category(json!([group(
            0,
            0.0,
            100.0,
            json!([entry(1, 0, 0.0), entry(2, 0, 0.0), entry(3, 1, 0.0)])
        )]));
        let r = resolve_category(&c, &authored_baseline_inputs(&c, 10)).unwrap();
        assert_eq!(r.rarity_counts, [2, 1, 0, 0]);
        assert_eq!(
            r.entries[0].weight_before_normalization,
            Some(0.755_f32 / 2.0)
        );
        assert_eq!(r.entries[2].weight_before_normalization, Some(0.22));
        assert_eq!(
            r.entries
                .iter()
                .map(|e| e.u24_numerator_count.unwrap())
                .sum::<u32>(),
            U24_DENOMINATOR
        );
    }
    #[test]
    fn positive_bias_subtracts_from_rarity_weight_and_floors_after_division() {
        let c = category(json!([group(
            0,
            0.0,
            100.0,
            json!([entry(1, 0, 0.755), entry(2, 0, 0.0)])
        )]));
        let r = resolve_category(&c, &authored_baseline_inputs(&c, 10)).unwrap();
        assert_eq!(
            r.entries[0].weight_before_normalization,
            Some(f32::MIN_POSITIVE)
        );
        assert_eq!(r.entries[0].u24_numerator_count, Some(1));
        assert!(r.entries[1].conditional_probability.unwrap() > 0.999999);
    }
    #[test]
    fn interpolation_flips_at_one_and_retains_negative_values() {
        let c = category(json!([group(
            0,
            0.0,
            100.0,
            json!([entry(1, 0, 0.0), entry(2, 1, 0.0)])
        )]));
        let mut inputs = authored_baseline_inputs(&c, 100);
        inputs.category_interpolation.as_mut().unwrap().flip_end = 2.0;
        let r = resolve_category(&c, &inputs).unwrap();
        assert_eq!(
            r.entries[0].weight_before_normalization,
            Some((1.0_f32 - 0.755) - 0.755 + 0.755)
        );
        inputs.category_interpolation.as_mut().unwrap().flip_end = -1.0;
        let r = resolve_category(&c, &inputs).unwrap();
        assert!(r.entries[0].weight_before_normalization.unwrap() > 1.0);
        assert_eq!(
            r.entries[1].weight_before_normalization,
            Some(f32::MIN_POSITIVE)
        );
    }
    #[test]
    fn group_override_and_degenerate_interpolation_follow_native_path() {
        let mut g = group(0, 2.0, 2.0, json!([entry(1, 0, 0.0)]));
        g["interpolated"] = json!(true);
        g["interpolation_parameters"] = json!([0.0, 0.1, 0.0, 0.2]);
        let c = category(json!([g]));
        let mut i = authored_baseline_inputs(&c, 2);
        i.category_interpolation = None;
        let r = resolve_category(&c, &i).unwrap();
        assert_eq!(r.entries[0].interpolated_bias, Some(0.2));
        assert_eq!(r.entries[0].interpolated_flip, Some(0.4));
    }
    #[test]
    fn unknown_entry_blocks_normalization_but_proven_exclusion_does_not() {
        let c = category(json!([group(
            0,
            0.0,
            100.0,
            json!([entry(1, 0, 0.0), entry(2, 1, 0.0)])
        )]));
        let mut i = authored_baseline_inputs(&c, 1);
        i.entry_evidence.remove(&2);
        let r = resolve_category(&c, &i).unwrap();
        assert_eq!(r.status, "unresolved");
        assert!(r
            .entries
            .iter()
            .all(|e| e.conditional_probability.is_none()));
        i.entry_evidence.insert(
            2,
            EntryEvidence {
                accepted: Some(false),
                runtime_rarity: None,
                source: "fixture".into(),
            },
        );
        let r = resolve_category(&c, &i).unwrap();
        assert_eq!(r.entries[0].conditional_probability, Some(1.0));
    }
    #[test]
    fn runtime_rarity_overrides_authored_and_invalid_rarity_fails() {
        let c = category(json!([group(0, 0.0, 100.0, json!([entry(1, 0, 0.0)]))]));
        let mut i = authored_baseline_inputs(&c, 1);
        i.entry_evidence.get_mut(&1).unwrap().runtime_rarity = Some(3);
        let r = resolve_category(&c, &i).unwrap();
        assert_eq!(r.rarity_counts, [0, 0, 0, 1]);
        i.entry_evidence.get_mut(&1).unwrap().runtime_rarity = Some(4);
        assert!(resolve_category(&c, &i).is_err());
    }
    #[test]
    fn native_u24_equality_zero_and_tail_fallback() {
        assert_eq!(
            u24_selection_counts(&[0.5, 0.5]).unwrap(),
            vec![8_388_609, 8_388_607]
        );
        assert_eq!(
            u24_selection_counts(&[0.0, 0.0, 1.0]).unwrap(),
            vec![1, 0, 16_777_215]
        );
        assert_eq!(
            u24_selection_counts(&[0.25, 0.25]).unwrap(),
            vec![4_194_305, 12_582_911]
        );
        assert_eq!(select_u24(&[0.5, 0.5], 8_388_608).unwrap(), 0);
        assert!(select_u24(&[1.0], U24_DENOMINATOR).is_err());
    }
    #[test]
    fn all_items_mode_does_not_publish_random_item_probabilities() {
        let mut c = category(json!([group(0, 0.0, 100.0, json!([entry(1, 0, 0.0)]))]));
        c.prefix_bytes[1] = 1;
        let r = resolve_category(&c, &authored_baseline_inputs(&c, 1)).unwrap();
        assert_eq!(r.status, "unresolved");
        assert!(r.entries[0].conditional_probability.is_none());
        c.prefix_bytes[1] = 0;
        let mut i = authored_baseline_inputs(&c, 1);
        i.random_selection_mode = None;
        assert_eq!(resolve_category(&c, &i).unwrap().status, "unresolved");
    }
    #[test]
    fn large_unsigned_level_is_converted_to_native_f32_before_filtering() {
        let c = category(json!([group(
            0,
            16_777_216.0,
            16_777_216.0,
            json!([entry(1, 0, 0.0)])
        )]));
        let r = resolve_category(&c, &authored_baseline_inputs(&c, 16_777_217)).unwrap();
        assert_eq!(r.eligible_group_indices, vec![0]);
    }
}
