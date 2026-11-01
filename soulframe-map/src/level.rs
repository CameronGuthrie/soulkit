//! Level layout with format tag 212: length-prefixed header records and an ordinally
//! paired body (one text length per entity, then NUL-terminated text blocks).
//! Positions are already world-space, including entities in prefab scopes.
use anyhow::{ensure, Context, Result};

#[derive(Debug)]
pub(crate) struct LevelEntity {
    pub index: usize,
    pub header_offset: usize,
    pub body_offset: usize,
    pub type_path: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scope: String,
    pub instance: u32,
    pub text: String,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .context("level offset overflow")?;
        let data = self
            .bytes
            .get(self.offset..end)
            .context("truncated level record")?;
        self.offset = end;
        Ok(data)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        Ok(std::str::from_utf8(self.take(len)?)?.to_owned())
    }
    fn strings(&mut self) -> Result<()> {
        let count = self.u32()?;
        for _ in 0..count {
            self.string()?;
        }
        Ok(())
    }
    fn floats<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut values = [0.0; N];
        for value in &mut values {
            *value = f32::from_bits(self.u32()?);
            ensure!(value.is_finite(), "non-finite entity transform");
        }
        Ok(values)
    }
}

pub(crate) fn parse_level(header: &[u8], body: &[u8]) -> Result<Vec<LevelEntity>> {
    let mut h = Reader {
        bytes: header,
        offset: 0,
    };
    h.take(16)?; // resource hash
    h.strings()?; // package dependencies
    let metadata_len = h.u32()? as usize;
    h.take(metadata_len)?;
    if metadata_len != 0 {
        h.take(1)?;
    }
    ensure!(
        h.u32()? == 212,
        "unsupported level format tag (expected 212)"
    );
    for _ in 0..3 {
        h.strings()?;
    } // resource, entity-type, material tables
    let variants = h.u32()?;
    for _ in 0..variants {
        h.string()?; // quest name
        h.string()?; // .tlevel path
        let layers = h.u32()?;
        h.take(
            (layers as usize)
                .checked_mul(4)
                .context("layer count overflow")?,
        )?;
    }
    ensure!(h.u32()? == 0, "unsupported level pre-entity section");
    let count = h.u32()? as usize;
    ensure!(
        count > 0 && count <= body.len() / 5,
        "invalid level entity count"
    );
    let mut b = Reader {
        bytes: body,
        offset: 0,
    };
    let sizes: Vec<_> = (0..count).map(|_| b.u32()).collect::<Result<_>>()?;
    // This also rejects stale H records paired with a newer B record.
    let total = sizes
        .iter()
        .try_fold(b.offset, |size, len| {
            size.checked_add(*len as usize)?.checked_add(1)
        })
        .context("body length overflow")?;
    ensure!(
        total == body.len(),
        "level header/body record lengths do not match"
    );
    let mut entities = Vec::with_capacity(count);
    for (index, size) in sizes.into_iter().enumerate() {
        let header_offset = h.offset;
        let type_path = h.string()?;
        ensure!(!type_path.is_empty(), "empty entity type at record {index}");
        let position = h.floats()?;
        let rotation = h.floats()?;
        let scope = h.string()?;
        ensure!(
            scope.starts_with('/') && scope.ends_with('/'),
            "invalid entity scope"
        );
        let instance = h.u32()?;
        ensure!(h.u32()? == 0, "unsupported entity flags at record {index}");
        let body_offset = b.offset;
        let text = std::str::from_utf8(b.take(size as usize)?)?.to_owned();
        ensure!(b.take(1)? == [0], "missing entity text terminator");
        ensure!(
            text.lines().next() == Some(format!("Instance={instance}").as_str()),
            "instance mismatch in level record {index}"
        );
        entities.push(LevelEntity {
            index,
            header_offset,
            body_offset,
            type_path,
            position,
            rotation,
            scope,
            instance,
            text,
        });
    }
    Ok(entities)
}

pub(crate) fn resource_metadata(header: &[u8]) -> Result<String> {
    let mut h = Reader {
        bytes: header,
        offset: 0,
    };
    h.take(16)?;
    h.strings()?;
    h.string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(bytes: &mut Vec<u8>, text: &str) {
        bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
    }
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let mut h = vec![0; 24];
        h.extend_from_slice(&212u32.to_le_bytes());
        h.extend_from_slice(&[0; 20]); // three tables, quest variants, reserved
        h.extend_from_slice(&2u32.to_le_bytes());
        let mut b = Vec::new();
        for _ in 0..2 {
            b.extend_from_slice(&11u32.to_le_bytes());
        }
        for (scope, x) in [("/Layer1/", 0.0f32), ("/Layer2/Prefab0/", 100.0)] {
            string(&mut h, "/SF/Types/Gameplay/LootChestSpawn");
            for value in [x, 250.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
                h.extend_from_slice(&value.to_le_bytes());
            }
            string(&mut h, scope);
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(b"Instance=0\n\0");
        }
        (h, b)
    }
    #[test]
    fn pairs_identical_types_and_instances_by_record_order() {
        let (h, b) = fixture();
        let entities = parse_level(&h, &b).unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].position, [0.0, 250.0, 0.0]);
        assert_eq!(entities[1].position, [100.0, 250.0, 0.0]);
        assert_eq!(entities[1].scope, "/Layer2/Prefab0/");
    }
    #[test]
    fn rejects_misaligned_truncated_and_nonfinite_records() {
        let (h, mut b) = fixture();
        b[0] += 1;
        assert!(parse_level(&h, &b).is_err());
        let (h, mut b) = fixture();
        b[17] = b'1'; // first Instance ID
        assert!(parse_level(&h, &b).is_err());
        let (mut h, b) = fixture();
        let entities = parse_level(&h, &b).unwrap();
        let pos = entities[0].header_offset + 4 + entities[0].type_path.len();
        h[pos..pos + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_level(&h, &b).is_err());
        assert!(parse_level(&h[..40], &b).is_err());
    }
}
