//! The common prefix every `H` (header) cache entry starts with.
//!
//! Layout: a 16-byte hash, a count-prefixed list of source paths, a
//! length-prefixed attribute text (followed by one padding byte when it is
//! non-empty), then a `u32` whose low 16 bits are the format type. The
//! format-specific header follows immediately after.

use anyhow::{anyhow, ensure, Result};

#[derive(Debug, Clone)]
pub struct CommonHeader {
    pub paths: Vec<String>,
    /// Raw attribute text. Usually ASCII "key=value" lines.
    pub attributes: Vec<u8>,
    /// Format type (`u32 & 0xFFFF`), e.g. 106 for a static mesh.
    pub type_id: u32,
    /// Offset of the first byte after the common header.
    pub end: usize,
}

fn read_u32(bin: &[u8], pos: &mut usize) -> Result<u32> {
    let bytes = bin
        .get(*pos..*pos + 4)
        .ok_or_else(|| anyhow!("Truncated common header"))?;
    *pos += 4;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_bytes<'a>(bin: &'a [u8], pos: &mut usize, len: u32) -> Result<&'a [u8]> {
    let end = pos
        .checked_add(len as usize)
        .filter(|&end| end <= bin.len())
        .ok_or_else(|| anyhow!("Truncated common header"))?;
    let bytes = &bin[*pos..end];
    *pos = end;
    Ok(bytes)
}

pub fn parse_common_header(bin: &[u8]) -> Result<CommonHeader> {
    ensure!(bin.len() >= 16, "Header too short");
    let mut pos = 16;
    let count = read_u32(bin, &mut pos)?;
    ensure!(
        count as usize <= (bin.len() - pos) / 4,
        "Truncated dependency table"
    );
    let mut paths = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = read_u32(bin, &mut pos)?;
        paths.push(String::from_utf8_lossy(read_bytes(bin, &mut pos, len)?).into_owned());
    }
    let len = read_u32(bin, &mut pos)?;
    let attributes = read_bytes(bin, &mut pos, len)?.to_vec();
    if len > 0 {
        pos += 1;
    }
    let type_id = read_u32(bin, &mut pos)? & 0xFFFF;
    Ok(CommonHeader {
        paths,
        attributes,
        type_id,
        end: pos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paths_attributes_and_masked_type() {
        let mut bin = vec![0u8; 16];
        bin.extend(1u32.to_le_bytes());
        bin.extend(3u32.to_le_bytes());
        bin.extend(b"a/b");
        bin.extend(2u32.to_le_bytes());
        bin.extend(b"x=");
        bin.push(0); // padding after non-empty attributes
        bin.extend(0x0001_006Au32.to_le_bytes());
        bin.push(0xAA);
        let header = parse_common_header(&bin).unwrap();
        assert_eq!(header.paths, ["a/b"]);
        assert_eq!(header.attributes, b"x=");
        assert_eq!(header.type_id, 106);
        assert_eq!(bin[header.end], 0xAA);
    }

    #[test]
    fn truncated_input_errors_instead_of_panicking() {
        let mut bin = vec![0u8; 16];
        bin.extend(u32::MAX.to_le_bytes());
        assert!(parse_common_header(&bin).is_err());
        assert!(parse_common_header(&[0; 8]).is_err());
    }
}
