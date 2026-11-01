//! LightMap bodies store mip levels smallest first, split between resident B
//! mips (the header's mip count) and the higher-resolution streamed F mips.
use anyhow::{bail, ensure, Result};
use soulframe_images::{dds_header_pack, texture_unpack_header};

pub(crate) fn tile_dds(header: &[u8], body: &[u8], streamed: bool) -> Result<Vec<u8>> {
    let mut info = texture_unpack_header(header)?;
    ensure!(info.width > 0 && info.height > 0, "zero-sized tile");
    let mut levels = Vec::new();
    let (mut w, mut h) = (info.width, info.height);
    loop {
        let size = match info.format {
            0 => usize::from(w) * usize::from(h) * 4,
            1 | 6 => usize::from(w).div_ceil(4) * usize::from(h).div_ceil(4) * 8,
            3 | 7 | 10 | 12 => usize::from(w).div_ceil(4) * usize::from(h).div_ceil(4) * 16,
            _ => bail!("unsupported tile format {}", info.format),
        };
        levels.push((w, h, size));
        if w == 1 && h == 1 {
            break;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    let resident = usize::from(info.miplevels);
    ensure!(
        resident > 0 && resident <= levels.len(),
        "invalid resident mip count"
    );
    let split = levels.len() - resident;
    let stored = if streamed {
        &levels[..split]
    } else {
        &levels[split..]
    };
    let &(width, height, size) = stored
        .first()
        .ok_or_else(|| anyhow::anyhow!("no streamed mips"))?;
    let expected: usize = stored.iter().map(|level| level.2).sum();
    ensure!(
        body.len() == expected,
        "incompatible LightMap body: format {}, expected {} bytes, got {}",
        info.format,
        expected,
        body.len()
    );
    info.width = width;
    info.height = height;
    info.data_size = size;
    info.miplevels = 1;
    let mut dds = dds_header_pack(&info);
    dds.extend_from_slice(&body[body.len() - size..]);
    Ok(dds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(format: u8, mips: u8) -> Vec<u8> {
        let mut bytes = vec![0; 24]; // hash, no dependencies, no metadata
        bytes.extend_from_slice(&164u32.to_le_bytes());
        bytes.extend_from_slice(&[mips, 1, 1, format]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&512u16.to_le_bytes());
        bytes.extend_from_slice(&512u16.to_le_bytes());
        bytes
    }

    #[test]
    fn rejects_stale_bc3_header_for_bc1_stream() {
        let body = vec![42; 131072];
        assert!(tile_dds(&header(3, 8), &body, true).is_err());
        let dds = tile_dds(&header(1, 9), &body, true).unwrap();
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 512);
        assert_eq!(&dds[128..], body);
    }

    #[test]
    fn selects_largest_mip_at_end_of_bc3_stream() {
        let mut body = vec![1; 65536]; // 256x256
        body.extend(vec![2; 262144]); // 512x512
        let dds = tile_dds(&header(3, 8), &body, true).unwrap();
        assert_eq!(&dds[84..88], b"DXT5");
        assert_eq!(&dds[128..], vec![2; 262144]);
        assert_eq!(u32::from_le_bytes(dds[28..32].try_into().unwrap()), 1);
    }

    #[test]
    fn validates_resident_chain_and_uses_its_actual_dimensions() {
        for (format, mips, bytes, width, top_size) in
            [(1, 9, 43704, 256, 32768), (3, 8, 21872, 128, 16384)]
        {
            let dds = tile_dds(&header(format, mips), &vec![7; bytes], false).unwrap();
            assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), width);
            assert_eq!(dds.len(), 128 + top_size);
            assert!(tile_dds(&header(format, mips), &vec![7; bytes - 1], false).is_err());
        }
    }
}
