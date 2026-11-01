//! Counted decoder for the current decrypted eight-category `SFDropTable` layout.
//!
//! This decoder never searches for resource strings or guesses category boundaries
//! from a level-range maximum. Other object types and unknown serialized variants
//! fail with a file offset. Item rarity/weight fields are authored evidence; they
//! are not a claim about runtime resource rarity or final selection probability.

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HEADER: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0, 14, 0, 0, 0];
const CATEGORY_COUNT: usize = 8;
const MAX_COLLECTION_COUNT: u32 = 10_000;
const MAX_RESOURCE_BYTES: u32 = 4_096;

/// Offsets in all structs are measured from the start of the complete `.bin`,
/// including its twelve-byte object header. End offsets are exclusive.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecodedDropTable {
    pub source_bytes: usize,
    pub source_sha256: String,
    pub object_header: [u8; 12],
    pub object_prefix: u32,
    pub categories: Vec<DropCategory>,
    pub category_end_offset: usize,
    pub undecoded_tail_bytes: usize,
    pub tail_sha256: String,
    /// The entire tail is retained for later decoding; no tail defaults are inferred.
    pub tail_hex: String,
    /// Only this first length-prefixed tail reference is currently validated.
    pub tail_prefix_resource: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DropCategory {
    pub index: usize,
    pub file_offset: usize,
    pub end_offset: usize,
    /// Preserved flags. Their presence must not silently disappear in a selector.
    pub prefix_bytes: [u8; 3],
    /// Optional upgrade-stat ID (native category +0x58), kept under the initial
    /// evidence API name. Observed values are 0 and 61; nonzero has a reference.
    pub reserved_field: u32,
    /// Native category +0x60. None differs from an explicitly empty reference.
    pub modifier_reference: Option<String>,
    pub chance_file_offset: usize,
    pub chance: f32,
    pub minimum_drops: u32,
    pub maximum_drops: u32,
    pub quality_indices: Vec<u32>,
    pub level_groups: Vec<LevelGroup>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelGroup {
    pub index: usize,
    pub file_offset: usize,
    pub end_offset: usize,
    pub interpolated: bool,
    /// Four serialized parameters, retained in authored order. No inferred defaults.
    pub interpolation_parameters: Option<[f32; 4]>,
    pub minimum_level: f32,
    pub maximum_level: f32,
    pub items: Vec<DropEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DropEntry {
    pub index: usize,
    pub file_offset: usize,
    pub end_offset: usize,
    pub reference_kind: u32,
    pub path: String,
    pub bias: f32,
    /// Authored serialized evidence, not the runtime resource-derived rarity.
    pub rarity_index: u32,
    /// Authored serialized evidence. Native collection can reset this value.
    pub authored_weight: f32,
}

impl DecodedDropTable {
    pub fn entry_count(&self) -> usize {
        self.categories
            .iter()
            .flat_map(|category| &category.level_groups)
            .map(|group| group.items.len())
            .sum()
    }
}

impl DropCategory {
    /// The decoded vector is validated as unique indices 0 through 6.
    pub fn quality_mask(&self) -> u8 {
        self.quality_indices
            .iter()
            .filter(|&&index| index <= 6)
            .fold(0, |mask, &index| mask | (1 << index))
    }
}

impl LevelGroup {
    /// Native collection converts the unsigned level to float32 before its two
    /// inclusive comparisons. This is only level eligibility, not resource acceptance.
    pub fn contains_level(&self, level: u32) -> bool {
        let level = level as f32;
        self.minimum_level <= level && level <= self.maximum_level
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(length)
            .context("serialized byte-count overflow")?;
        let bytes = self.data.get(self.pos..end).with_context(|| {
            format!(
                "truncated data at file offset {:#x}: need {} bytes, {} remain",
                self.pos,
                length,
                self.data.len().saturating_sub(self.pos)
            )
        })?;
        self.pos = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32> {
        let start = self.pos;
        let value = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        ensure!(
            value.is_finite(),
            "nonfinite float at file offset {start:#x}"
        );
        Ok(value)
    }

    fn count(&mut self, maximum: u32) -> Result<usize> {
        let start = self.pos;
        let count = self.u32()?;
        ensure!(
            count <= maximum,
            "unsupported count {count} at file offset {start:#x} (maximum {maximum})"
        );
        Ok(count as usize)
    }

    fn string(&mut self) -> Result<String> {
        let start = self.pos;
        let count = self.count(MAX_RESOURCE_BYTES)?;
        let bytes = self.take(count)?;
        let text = std::str::from_utf8(bytes)
            .with_context(|| format!("invalid resource UTF-8 at file offset {start:#x}"))?;
        ensure!(
            !text.chars().any(char::is_control),
            "control character in resource at file offset {start:#x}"
        );
        Ok(text.to_owned())
    }
}

fn resource_path(path: &str) -> bool {
    path.starts_with("/SF/") || path.starts_with("/Lotus/")
}

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Decode the observed `SFDropTable` serialized layout, retaining all category,
/// level-group and item boundaries. This accepts the whole decrypted file, not
/// the slice after its header. A failure returns no partial category list.
pub fn decode_categories(data: &[u8]) -> Result<DecodedDropTable> {
    let mut r = Reader { data, pos: 0 };
    let object_header: [u8; 12] = r.take(12)?.try_into().unwrap();
    ensure!(
        object_header == HEADER,
        "unsupported decrypted object header at file offset 0x0: {}",
        hex::encode(object_header)
    );
    let object_prefix = r.u32()?;
    ensure!(
        object_prefix == 0,
        "unsupported object prefix {object_prefix} at file offset 0xc"
    );

    let mut categories = Vec::with_capacity(CATEGORY_COUNT);
    for index in 0..CATEGORY_COUNT {
        let file_offset = r.pos;
        let prefix_bytes: [u8; 3] = r.take(3)?.try_into().unwrap();
        ensure!(
            prefix_bytes.iter().all(|&flag| flag <= 1),
            "unsupported category prefix at file offset {file_offset:#x}"
        );
        let chance_file_offset = r.pos;
        let chance = r.f32()?;
        let minimum_drops = r.u32()?;
        let maximum_drops = r.u32()?;
        ensure!(
            (0.0..=1.0).contains(&chance)
                && minimum_drops <= maximum_drops
                && maximum_drops <= MAX_COLLECTION_COUNT,
            "unsupported category chance/count bounds at file offset {chance_file_offset:#x}"
        );

        let quality_count = r.count(7)?;
        let mut quality_indices = Vec::with_capacity(quality_count);
        for _ in 0..quality_count {
            let offset = r.pos;
            let quality = r.u32()?;
            ensure!(
                quality <= 6 && !quality_indices.contains(&quality),
                "invalid or duplicate quality index {quality} at file offset {offset:#x}"
            );
            quality_indices.push(quality);
        }

        let reserved_offset = r.pos;
        let reserved_field = r.u32()?;
        let modifier_reference = match reserved_field {
            0 => None,
            61 => {
                let path = r.string()?;
                ensure!(
                    path.is_empty() || resource_path(&path),
                    "unsupported modifier reference at file offset {reserved_offset:#x}"
                );
                Some(path)
            }
            value => {
                bail!("unsupported category field {value} at file offset {reserved_offset:#x}")
            }
        };

        let group_count = r.count(MAX_COLLECTION_COUNT)?;
        let mut level_groups = Vec::with_capacity(group_count);
        for group_index in 0..group_count {
            let group_offset = r.pos;
            let group_flag = r.u8()?;
            ensure!(
                group_flag <= 1,
                "unsupported level-group flag {group_flag} at file offset {group_offset:#x}"
            );
            let interpolation_parameters = if group_flag == 1 {
                Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?])
            } else {
                None
            };
            let minimum_level = r.f32()?;
            let maximum_level = r.f32()?;
            ensure!(
                minimum_level <= maximum_level,
                "reversed level range at file offset {group_offset:#x}"
            );
            let item_count = r.count(MAX_COLLECTION_COUNT)?;
            let mut items = Vec::with_capacity(item_count);
            for item_index in 0..item_count {
                let item_offset = r.pos;
                let reference_kind = r.u32()?;
                ensure!(
                    reference_kind <= 2,
                    "unsupported reference kind {reference_kind} at file offset {item_offset:#x}"
                );
                let path = r.string()?;
                ensure!(
                    resource_path(&path),
                    "unsupported item resource at file offset {item_offset:#x}"
                );
                let bias = r.f32()?;
                let rarity_index = r.u32()?;
                let authored_weight = r.f32()?;
                ensure!(
                    rarity_index <= 3 && authored_weight >= 0.0,
                    "invalid authored rarity/weight at file offset {item_offset:#x}"
                );
                items.push(DropEntry {
                    index: item_index,
                    file_offset: item_offset,
                    end_offset: r.pos,
                    reference_kind,
                    path,
                    bias,
                    rarity_index,
                    authored_weight,
                });
            }
            level_groups.push(LevelGroup {
                index: group_index,
                file_offset: group_offset,
                end_offset: r.pos,
                interpolated: group_flag == 1,
                interpolation_parameters,
                minimum_level,
                maximum_level,
                items,
            });
        }

        categories.push(DropCategory {
            index,
            file_offset,
            end_offset: r.pos,
            prefix_bytes,
            reserved_field,
            modifier_reference,
            chance_file_offset,
            chance,
            minimum_drops,
            maximum_drops,
            quality_indices,
            level_groups,
        });
    }

    let category_end_offset = r.pos;
    let tail_prefix_resource = r.string()?;
    ensure!(
        tail_prefix_resource.starts_with("/Lotus/Types/Items/"),
        "unexpected post-category resource at file offset {category_end_offset:#x}"
    );
    let tail = &data[category_end_offset..];
    Ok(DecodedDropTable {
        source_bytes: data.len(),
        source_sha256: sha256(data),
        object_header,
        object_prefix,
        categories,
        category_end_offset,
        undecoded_tail_bytes: tail.len(),
        tail_sha256: sha256(tail),
        tail_hex: hex::encode(tail),
        tail_prefix_resource,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_bytes(out: &mut Vec<u8>, value: u32) {
        out.extend(value.to_le_bytes());
    }

    fn float(out: &mut Vec<u8>, value: f32) {
        out.extend(value.to_le_bytes());
    }

    fn string(out: &mut Vec<u8>, value: &str) {
        u32_bytes(out, value.len() as u32);
        out.extend(value.as_bytes());
    }

    fn group(
        minimum: f32,
        maximum: f32,
        paths: &[&str],
        interpolation: Option<[f32; 4]>,
    ) -> Vec<u8> {
        let mut out = vec![u8::from(interpolation.is_some())];
        if let Some(values) = interpolation {
            for value in values {
                float(&mut out, value);
            }
        }
        float(&mut out, minimum);
        float(&mut out, maximum);
        u32_bytes(&mut out, paths.len() as u32);
        for path in paths {
            u32_bytes(&mut out, 0);
            string(&mut out, path);
            float(&mut out, 0.125);
            u32_bytes(&mut out, 2);
            float(&mut out, 0.03125); // retain noncanonical authored weight
        }
        out
    }

    fn category(
        chance: f32,
        counts: [u32; 2],
        groups: Vec<Vec<u8>>,
        modifier: Option<&str>,
    ) -> Vec<u8> {
        let mut out = vec![0, 0, 0];
        float(&mut out, chance);
        for value in counts {
            u32_bytes(&mut out, value);
        }
        u32_bytes(&mut out, 5);
        for quality in 2..=6 {
            u32_bytes(&mut out, quality);
        }
        u32_bytes(&mut out, if modifier.is_some() { 61 } else { 0 });
        if let Some(reference) = modifier {
            string(&mut out, reference);
        }
        u32_bytes(&mut out, groups.len() as u32);
        for group in groups {
            out.extend(group);
        }
        out
    }

    fn fixture() -> Vec<u8> {
        let mut out = HEADER.to_vec();
        u32_bytes(&mut out, 0);
        let categories = [
            category(
                0.55,
                [2, 4],
                vec![group(0.0, 100.0, &["/SF/Currency"], None)],
                None,
            ),
            category(
                1.0,
                [1, 1],
                vec![group(0.0, 100.0, &["/SF/Talisman"], None)],
                None,
            ),
            category(
                0.1,
                [0, 3],
                vec![
                    group(0.0, 25.0, &["/SF/CatalystA"], None),
                    group(20.0, 100.0, &["/SF/CatalystB"], None),
                ],
                None,
            ),
            category(
                0.65,
                [0, 1],
                vec![group(
                    25.0,
                    100.0,
                    &["/SF/Kintsugi"],
                    Some([0.0, 1.0, 2.0, 3.0]),
                )],
                None,
            ),
            category(
                0.3,
                [0, 1],
                vec![],
                Some("/SF/Types/Items/Consumables/RedNightUpgrade"),
            ),
            category(
                0.5,
                [0, 1],
                vec![group(0.0, 100.0, &["/SF/TokenA", "/SF/TokenB"], None)],
                Some(""),
            ),
            category(
                0.3,
                [0, 1],
                vec![group(0.0, 100.0, &["/SF/Rune"], None)],
                None,
            ),
            category(
                0.8,
                [0, 1],
                vec![
                    group(0.0, 100.0, &["/SF/DefaultWeapon"], None),
                    group(0.0, 15.0, &["/SF/LowWeapon"], None),
                    group(15.0, 100.0, &["/SF/HighWeapon"], None),
                ],
                None,
            ),
        ];
        for category in categories {
            out.extend(category);
        }
        string(&mut out, "/Lotus/Types/Items/DefaultStrippedItem");
        out.extend([0xde, 0xad, 0xbe, 0xef]);
        out
    }

    #[test]
    fn preserves_non_100_ranges_empty_categories_and_independent_chances() {
        let table = decode_categories(&fixture()).unwrap();
        assert_eq!(table.categories.len(), 8);
        let talisman = &table.categories[1];
        assert_eq!((talisman.minimum_drops, talisman.maximum_drops), (1, 1));
        assert_eq!(talisman.level_groups[0].items[0].path, "/SF/Talisman");
        assert_eq!(talisman.level_groups[0].items.len(), 1);
        assert_eq!(table.categories[2].level_groups[0].maximum_level, 25.0);
        assert!(table.categories[4].level_groups.is_empty());
        assert_eq!(table.categories[5].chance, 0.5);
        assert_eq!(table.categories[5].level_groups[0].items.len(), 2);
        assert_eq!(table.categories[7].chance, 0.8);
        assert_eq!(table.categories[7].quality_mask(), 0x7c);
        let item = &talisman.level_groups[0].items[0];
        assert_eq!(item.bias, 0.125);
        assert_eq!(item.authored_weight, 0.03125);
        assert_eq!(table.entry_count(), 11);
    }

    #[test]
    fn preserves_interpolation_parameters_and_optional_reference_distinctions() {
        let table = decode_categories(&fixture()).unwrap();
        let group = &table.categories[3].level_groups[0];
        assert!(group.interpolated);
        assert_eq!(group.interpolation_parameters, Some([0.0, 1.0, 2.0, 3.0]));
        assert_eq!((group.minimum_level, group.maximum_level), (25.0, 100.0));
        assert_eq!(table.categories[3].modifier_reference, None);
        assert_eq!(
            table.categories[4].modifier_reference.as_deref(),
            Some("/SF/Types/Items/Consumables/RedNightUpgrade")
        );
        assert_eq!(table.categories[5].modifier_reference.as_deref(), Some(""));
        assert_eq!(table.categories[5].reserved_field, 61);
    }

    #[test]
    fn level_overlap_includes_endpoints_and_uses_native_f32_conversion() {
        let table = decode_categories(&fixture()).unwrap();
        let groups = &table.categories[7].level_groups;
        let active = |level| {
            groups
                .iter()
                .map(|group| group.contains_level(level))
                .collect::<Vec<_>>()
        };
        assert_eq!(active(14), [true, true, false]);
        assert_eq!(active(15), [true, true, true]);
        assert_eq!(active(35), [true, false, true]);
        let group = LevelGroup {
            minimum_level: 16_777_216.0,
            maximum_level: 16_777_216.0,
            ..Default::default()
        };
        assert!(group.contains_level(16_777_217));
        assert!(!group.contains_level(16_777_218));
    }

    #[test]
    fn offsets_and_tail_hash_preserve_full_file_evidence() {
        let data = fixture();
        let table = decode_categories(&data).unwrap();
        assert_eq!(table.source_bytes, data.len());
        assert_eq!(table.source_sha256, sha256(&data));
        assert_eq!(table.categories[0].file_offset, 16);
        for pair in table.categories.windows(2) {
            assert_eq!(pair[0].end_offset, pair[1].file_offset);
        }
        assert_eq!(table.categories[7].end_offset, table.category_end_offset);
        assert_eq!(
            hex::decode(&table.tail_hex).unwrap(),
            data[table.category_end_offset..]
        );
        assert_eq!(
            table.tail_sha256,
            sha256(&data[table.category_end_offset..])
        );
        assert_eq!(
            table.undecoded_tail_bytes,
            data.len() - table.category_end_offset
        );
        assert_eq!(
            table.tail_prefix_resource,
            "/Lotus/Types/Items/DefaultStrippedItem"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            serde_json::from_str::<DecodedDropTable>(&serde_json::to_string(&table).unwrap())
                .unwrap(),
            table
        );
    }

    #[test]
    fn truncated_count_or_entry_does_not_resynchronize_at_later_paths() {
        let original = fixture();
        let table = decode_categories(&original).unwrap();
        for length in [
            0,
            11,
            15,
            100,
            table.categories[5].level_groups[0].items[1].file_offset + 9,
            table.category_end_offset + 8,
        ] {
            let error = decode_categories(&original[..length]).unwrap_err();
            assert!(format!("{error:#}").contains("file offset"), "{error:#}");
        }
        let mut oversized = original.clone();
        let group = &table.categories[2].level_groups[0];
        oversized[group.file_offset + 9..group.file_offset + 13]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_categories(&oversized).is_err());
    }

    #[test]
    fn rejects_unknown_variants_and_nonfinite_values() {
        let original = fixture();
        let table = decode_categories(&original).unwrap();
        let mut header = original.clone();
        header[8] = 10; // a different manifest object, not an empty drop table
        assert!(decode_categories(&header)
            .unwrap_err()
            .to_string()
            .contains("object header"));
        let mut flags = original.clone();
        flags[table.categories[0].file_offset] = 2;
        assert!(decode_categories(&flags).is_err());
        let mut group_flag = original.clone();
        group_flag[table.categories[0].level_groups[0].file_offset] = 2;
        assert!(decode_categories(&group_flag).is_err());
        let mut reference = original.clone();
        let offset = table.categories[0].level_groups[0].items[0].file_offset;
        reference[offset..offset + 4].copy_from_slice(&3_u32.to_le_bytes());
        assert!(decode_categories(&reference).is_err());
        let mut nonfinite = original;
        let offset = table.categories[7].chance_file_offset;
        nonfinite[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_categories(&nonfinite).is_err());
    }

    #[test]
    fn category_flags_and_ref_kinds_are_preserved_not_normalized() {
        let mut data = fixture();
        let baseline = decode_categories(&data).unwrap();
        data[baseline.categories[6].file_offset..baseline.categories[6].file_offset + 3]
            .copy_from_slice(&[1, 1, 1]);
        let entry = &baseline.categories[6].level_groups[0].items[0];
        data[entry.file_offset..entry.file_offset + 4].copy_from_slice(&2_u32.to_le_bytes());
        let decoded = decode_categories(&data).unwrap();
        assert_eq!(decoded.categories[6].prefix_bytes, [1, 1, 1]);
        assert_eq!(
            decoded.categories[6].level_groups[0].items[0].reference_kind,
            2
        );
    }

    #[test]
    fn quality_vector_rejects_duplicates_and_unknown_indices() {
        let original = fixture();
        let table = decode_categories(&original).unwrap();
        // chance + min + max + count precede the vector.
        let first = table.categories[7].chance_file_offset + 16;
        for value in [3_u32, 7] {
            let mut data = original.clone();
            data[first..first + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode_categories(&data).is_err());
        }
    }
}
