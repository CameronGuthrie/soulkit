//! Soulframe image/texture decoding.
//!
//! This crate only owns the *format-specific* code: detecting image extensions
//! in TOC paths, parsing the proprietary texture header chunk, reconstructing
//! a DDS file from an `H` (header) + `B`/`F` (body) pair, optional Morton
//! deswizzle for `.Texture` caches, and an optional DDS→PNG step. All
//! cache / TOC / Oodle plumbing lives in [`soulframe_cache`].

pub mod cli;

use anyhow::{anyhow, ensure, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

/// Texture information extracted from a header chunk.
#[derive(Debug, Clone)]
pub struct TextureInfo {
    pub metadata: Vec<u8>,
    pub miplevels: u8,
    pub format: u8,
    pub width: u16,
    pub height: u16,
    pub data_size: usize,
}

/// Image file type detected from a path extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageType {
    /// `.png` files — in Soulframe caches these are actually DDS textures
    /// with a `.png` extension that need H+B reassembly.
    Png,
    /// `.jpg`/`.jpeg` files — also usually DDS-backed.
    Jpg,
    Dds,
    Tga,
    Bmp,
    Unknown,
}

impl ImageType {
    pub fn from_extension(path: &str) -> Self {
        let lower = path.to_lowercase();
        if lower.ends_with(".png") {
            ImageType::Png
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            ImageType::Jpg
        } else if lower.ends_with(".dds") {
            ImageType::Dds
        } else if lower.ends_with(".tga") {
            ImageType::Tga
        } else if lower.ends_with(".bmp") {
            ImageType::Bmp
        } else {
            ImageType::Unknown
        }
    }
}

pub fn is_image_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".dds")
        || lower.ends_with(".tga")
        || lower.ends_with(".bmp")
        || lower.ends_with(".hdr")
        || lower.ends_with(".exr")
}

/// Parse the H-chunk header that precedes a Soulframe texture body.
pub fn texture_unpack_header(bin: &[u8]) -> Result<TextureInfo> {
    texture_layout(bin).map(|(info, _)| info)
}

fn texture_layout(bin: &[u8]) -> Result<(TextureInfo, u8)> {
    let header = soulframe_cache::parse_common_header(bin)?;
    let metadata = header.attributes;
    let mut cursor = Cursor::new(bin);
    cursor.set_position(header.end as u64);

    let miplevels = cursor.read_u8()?;
    let streamed_mips = cursor.read_u8()?;
    let _unk2 = cursor.read_u8()?;
    let format = cursor.read_u8()?;
    let num_unk = cursor.read_u32::<LittleEndian>()?;
    let end = cursor
        .position()
        .checked_add(u64::from(num_unk) * 4)
        .ok_or_else(|| anyhow!("Texture table offset overflow"))?;
    ensure!(end <= bin.len() as u64, "Truncated texture table");
    cursor.set_position(end);

    let width = cursor.read_u16::<LittleEndian>()?;
    let height = cursor.read_u16::<LittleEndian>()?;
    ensure!(width > 0 && height > 0, "Zero-sized texture");

    // Format ↦ (bytes_per_block_or_pixel, is_compressed)
    //  0 = uncompressed RGBA8 (4 bpp)
    //  1 = BC1/DXT1   (8 B/block)
    //  3 = BC3/DXT5   (16 B/block)
    //  6 = BC4        (8 B/block, single channel)
    //  7 = BC5        (16 B/block, dual channel — normal maps)
    // 10 = BC6H       (16 B/block, HDR)
    // 12 = BC7        (16 B/block, high quality)
    let (bytes_per_block, is_compressed) = match format {
        0 => (4usize, false),
        1 => (8, true),
        3 => (16, true),
        6 => (8, true),
        7 => (16, true),
        10 => (16, true),
        12 => (16, true),
        _ => {
            return Err(anyhow!(
                "Unsupported texture format {} for {}x{} texture",
                format,
                width,
                height
            ));
        }
    };

    let data_size = if is_compressed {
        ((width as usize + 3) / 4).max(1) * ((height as usize + 3) / 4).max(1) * bytes_per_block
    } else {
        width as usize * height as usize * bytes_per_block
    };

    Ok((
        TextureInfo {
            metadata,
            miplevels,
            format,
            width,
            height,
            data_size,
        },
        streamed_mips,
    ))
}

/// Build a DDS header for the given texture info.
pub fn dds_header_pack(info: &TextureInfo) -> Vec<u8> {
    let mut bin = Vec::new();

    bin.extend_from_slice(b"DDS ");
    // dwSize = 124
    bin.extend_from_slice(&[0x7C, 0x00, 0x00, 0x00]);
    // dwFlags = DDSD_CAPS|HEIGHT|WIDTH|PIXELFORMAT|MIPMAPCOUNT|LINEARSIZE
    bin.extend_from_slice(&[0x07, 0x10, 0x0A, 0x00]);
    bin.extend_from_slice(&(info.height as u32).to_le_bytes());
    bin.extend_from_slice(&(info.width as u32).to_le_bytes());

    let first_mip_size = match info.format {
        0 => info.width as u32 * 4,
        1 | 6 => ((info.width + 3) / 4).max(1) as u32 * ((info.height + 3) / 4).max(1) as u32 * 8,
        3 | 7 | 10 | 12 => {
            ((info.width + 3) / 4).max(1) as u32 * ((info.height + 3) / 4).max(1) as u32 * 16
        }
        _ => ((info.width + 3) / 4).max(1) as u32 * ((info.height + 3) / 4).max(1) as u32 * 8,
    };
    bin.extend_from_slice(&first_mip_size.to_le_bytes());

    // dwDepth = 0
    bin.extend_from_slice(&[0; 4]);
    bin.extend_from_slice(&(info.miplevels as u32).to_le_bytes());
    // dwReserved1[11]
    bin.extend_from_slice(&[0; 44]);

    // DDS_PIXELFORMAT (32 B)
    bin.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]); // dwSize = 32
    match info.format {
        0 => {
            // Uncompressed RGBA8 — DDPF_RGB | DDPF_ALPHAPIXELS
            bin.extend_from_slice(&[0x41, 0x00, 0x00, 0x00]);
            bin.extend_from_slice(&[0; 4]); // dwFourCC unused
            bin.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]); // 32 bpp
            bin.extend_from_slice(&[0x00, 0x00, 0xFF, 0x00]); // R mask
            bin.extend_from_slice(&[0x00, 0xFF, 0x00, 0x00]); // G mask
            bin.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]); // B mask
            bin.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF]); // A mask
        }
        1 | 3 | 6 | 7 | 10 | 12 => {
            // DDPF_FOURCC
            bin.extend_from_slice(&[0x04, 0x00, 0x00, 0x00]);
            let fourcc: &[u8; 4] = match info.format {
                1 => b"DXT1",
                3 => b"DXT5",
                6 => b"ATI1",
                7 => b"ATI2",
                10 => b"DX10",
                12 => b"DX10",
                _ => b"DXT1",
            };
            bin.extend_from_slice(fourcc);
            bin.extend_from_slice(&[0; 20]);
        }
        _ => {
            bin.extend_from_slice(&[0x04, 0x00, 0x00, 0x00]);
            bin.extend_from_slice(b"DXT1");
            bin.extend_from_slice(&[0; 20]);
        }
    }

    // dwCaps
    let caps: u32 = if info.miplevels > 1 {
        0x0040_1008
    } else {
        0x0000_1000
    };
    bin.extend_from_slice(&caps.to_le_bytes());
    // dwCaps2/3/4
    bin.extend_from_slice(&[0; 12]);
    // dwReserved2
    bin.extend_from_slice(&[0; 4]);

    // DDS_HEADER_DXT10 extension for BC6H / BC7.
    if info.format == 10 || info.format == 12 {
        let dxgi_format: u32 = match info.format {
            10 => 95, // DXGI_FORMAT_BC6H_UF16
            12 => 98, // DXGI_FORMAT_BC7_UNORM
            _ => 0,
        };
        bin.extend_from_slice(&dxgi_format.to_le_bytes());
        bin.extend_from_slice(&3u32.to_le_bytes()); // D3D10_RESOURCE_DIMENSION_TEXTURE2D
        bin.extend_from_slice(&0u32.to_le_bytes()); // misc flag
        bin.extend_from_slice(&1u32.to_le_bytes()); // array size
        bin.extend_from_slice(&0u32.to_le_bytes()); // misc flags2
    }

    bin
}

/// Build a complete DDS file from an `H` (header) + `B`/`F` (body) pair.
///
/// When `fix_dims` is set and the body is smaller than the header declares,
/// the header dimensions are halved until the main mip fits — required for
/// the streamed cache data where only some mip levels are present.
pub fn texture_export_dds(h_data: &[u8], b_data: &[u8], fix_dims: bool) -> Result<Vec<u8>> {
    let mut info = texture_unpack_header(h_data)?;

    if fix_dims && b_data.len() < info.data_size {
        let (bytes_per_block, is_compressed) = match info.format {
            0 => (4usize, false),
            1 => (8, true),
            3 => (16, true),
            6 => (8, true),
            7 => (16, true),
            10 => (16, true),
            12 => (16, true),
            _ => (0, false),
        };

        if bytes_per_block > 0 {
            let mut w = info.width;
            let mut h = info.height;
            let calc_size = |w: u16, h: u16| -> usize {
                if is_compressed {
                    ((w as usize + 3) / 4).max(1) * ((h as usize + 3) / 4).max(1) * bytes_per_block
                } else {
                    w as usize * h as usize * bytes_per_block
                }
            };

            let min_dim: u16 = if is_compressed { 4 } else { 1 };
            while calc_size(w, h) > b_data.len() && w > min_dim && h > min_dim {
                w = (w / 2).max(min_dim);
                h = (h / 2).max(min_dim);
            }

            let new_size = calc_size(w, h);
            if new_size <= b_data.len() && (w != info.width || h != info.height) {
                eprintln!(
                    "INFO: Adjusting dimensions from {}x{} to {}x{} (body {} bytes, declared {} bytes)",
                    info.width, info.height, w, h, b_data.len(), info.data_size
                );
                info.width = w;
                info.height = h;
                info.data_size = new_size;
                info.miplevels = 1;
            }
        }
    }

    let body_to_use = if b_data.len() >= info.data_size {
        let start = b_data.len() - info.data_size;
        &b_data[start..start + info.data_size]
    } else {
        b_data
    };

    let mut result = dds_header_pack(&info);
    result.extend_from_slice(body_to_use);
    Ok(result)
}

/// Decode a selected texture only when the complete resident/streamed mip
/// layouts identify its format and dimensions. Some Texture headers retain
/// source format 0 even though their cached B/F mip chains are BC1. Infer BC1
/// only from an exact resident-chain match; genuine RGBA remains RGBA.
pub fn texture_export_selected_dds(
    header: &[u8],
    resident: Option<&[u8]>,
    streamed: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let (mut info, streamed_mips) = texture_layout(header)?;
    let mut dimensions = Vec::new();
    let (mut width, mut height) = (info.width, info.height);
    loop {
        dimensions.push((width, height));
        if width == 1 && height == 1 {
            break;
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    let resident_mips = usize::from(info.miplevels);
    ensure!(
        resident_mips > 0 && resident_mips <= dimensions.len(),
        "Invalid resident texture mip count"
    );
    let split = dimensions.len() - resident_mips;
    ensure!(
        usize::from(streamed_mips) <= split,
        "Invalid streamed texture mip count"
    );
    let resident_dims = &dimensions[split..];
    let streamed_dims = &dimensions[split - usize::from(streamed_mips)..split];
    let mut formats = vec![info.format];
    if info.format == 0 && resident.is_some() {
        formats.push(1);
    }
    let mut valid = Vec::new();
    for format in formats {
        let size = |&(w, h): &(u16, u16)| -> usize {
            match format {
                0 => usize::from(w) * usize::from(h) * 4,
                1 | 6 => usize::from(w).div_ceil(4) * usize::from(h).div_ceil(4) * 8,
                _ => usize::from(w).div_ceil(4) * usize::from(h).div_ceil(4) * 16,
            }
        };
        if resident
            .is_some_and(|bytes| bytes.len() != resident_dims.iter().map(size).sum::<usize>())
        {
            continue;
        }
        if streamed
            .is_some_and(|bytes| bytes.len() != streamed_dims.iter().map(size).sum::<usize>())
        {
            continue;
        }
        let (body, dims) = match (streamed, resident) {
            (Some(bytes), _) if !bytes.is_empty() && !streamed_dims.is_empty() => {
                (bytes, streamed_dims)
            }
            (_, Some(bytes)) if !bytes.is_empty() => (bytes, resident_dims),
            _ => continue,
        };
        let &(width, height) = dims.first().context("No texture mip data")?;
        let top_bytes = size(&(width, height));
        valid.push((format, width, height, top_bytes, body));
    }
    ensure!(valid.len() == 1, "Texture mip layout is unsupported, truncated or ambiguous ({} matching formats); preserve original cache data", valid.len());
    let (format, width, height, top_bytes, body) = valid.pop().unwrap();
    info.format = format;
    info.width = width;
    info.height = height;
    info.data_size = top_bytes;
    info.miplevels = 1;
    let mut dds = dds_header_pack(&info);
    dds.extend_from_slice(&body[body.len() - top_bytes..]);
    Ok(dds)
}

/// Morton (Z-order) encoding for swizzled textures.
fn morton_encode(x: u32, y: u32) -> u32 {
    let mut result = 0u32;
    for i in 0..16 {
        result |= ((x & (1 << i)) << i) | ((y & (1 << i)) << (i + 1));
    }
    result
}

/// Deswizzle a DDS file with Morton-order swizzled pixel data. Caller is
/// responsible for restricting this to `.Texture` caches — `.LightMap` data
/// is not Morton-ordered and would be corrupted by this transform.
pub fn dds_deswizzle(dds_data: &[u8]) -> Result<Vec<u8>> {
    if dds_data.len() < 128 {
        return Err(anyhow!("DDS file too small"));
    }
    if &dds_data[0..4] != b"DDS " {
        return Err(anyhow!("Not a DDS file"));
    }

    let height = u32::from_le_bytes([dds_data[12], dds_data[13], dds_data[14], dds_data[15]]);
    let width = u32::from_le_bytes([dds_data[16], dds_data[17], dds_data[18], dds_data[19]]);

    let fourcc = &dds_data[84..88];
    let block_size = match fourcc {
        b"DXT1" | b"ATI1" => 8,
        b"DXT5" | b"ATI2" => 16,
        b"DX10" => 16,
        _ => return Ok(dds_data.to_vec()),
    };

    let blocks_wide = (width + 3) / 4;
    let blocks_high = (height + 3) / 4;
    let total_blocks = (blocks_wide * blocks_high) as usize;

    let header_size = if fourcc == b"DX10" { 128 + 20 } else { 128 };
    let expected_data_size = total_blocks * block_size;

    if dds_data.len() < header_size + expected_data_size {
        return Ok(dds_data.to_vec());
    }

    let swizzled_data = &dds_data[header_size..header_size + expected_data_size];
    let mut linear_data = vec![0u8; expected_data_size];

    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let swizzled_index = morton_encode(block_x, block_y) as usize;
            let linear_index = (block_y * blocks_wide + block_x) as usize;

            if swizzled_index < total_blocks && linear_index < total_blocks {
                let src_offset = swizzled_index * block_size;
                let dst_offset = linear_index * block_size;
                if src_offset + block_size <= swizzled_data.len() {
                    linear_data[dst_offset..dst_offset + block_size]
                        .copy_from_slice(&swizzled_data[src_offset..src_offset + block_size]);
                }
            }
        }
    }

    let mut result = Vec::with_capacity(header_size + linear_data.len());
    result.extend_from_slice(&dds_data[0..header_size]);
    result.extend_from_slice(&linear_data);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_header(width: u16, height: u16, resident: u8, streamed: u8, format: u8) -> Vec<u8> {
        let mut bytes = vec![0; 40];
        bytes[24..28].copy_from_slice(&163u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&[resident, streamed, 1, format]);
        bytes[36..38].copy_from_slice(&width.to_le_bytes());
        bytes[38..40].copy_from_slice(&height.to_le_bytes());
        bytes
    }

    fn mip_chain(mut width: usize, mut height: usize, count: usize, format: u8) -> Vec<u8> {
        let mut mips = Vec::new();
        for level in 0..count {
            let size = match format {
                0 => width * height * 4,
                1 => width.div_ceil(4) * height.div_ceil(4) * 8,
                _ => width.div_ceil(4) * height.div_ceil(4) * 16,
            };
            mips.push(vec![(level + 1) as u8; size]);
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        mips.into_iter().rev().flatten().collect()
    }

    #[test]
    fn selected_texture_infers_bc1_from_exact_resident_and_streamed_mip_chains() {
        // Source dimensions can exceed the installed streamed texture. Two F
        // levels above a 16px resident chain produce a 64px export, not 256px.
        let header = selected_header(256, 256, 5, 2, 0);
        let resident = mip_chain(16, 16, 5, 1);
        let streamed = mip_chain(64, 64, 2, 1);
        let dds = texture_export_selected_dds(&header, Some(&resident), Some(&streamed)).unwrap();
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 64);
        assert_eq!(dds.len(), 128 + 64 * 64 / 2);
        assert!(dds[128..].iter().all(|&byte| byte == 1));
        assert!(texture_export_selected_dds(
            &header,
            Some(&resident),
            Some(&streamed[..streamed.len() - 1])
        )
        .is_err());
        assert!(texture_export_selected_dds(&header, None, Some(&streamed)).is_err());
    }

    #[test]
    fn selected_texture_preserves_real_rgba_and_valid_bc3_resident_fallbacks() {
        let rgba = selected_header(64, 64, 5, 2, 0);
        let resident = mip_chain(16, 16, 5, 0);
        let streamed = mip_chain(64, 64, 2, 0);
        let dds = texture_export_selected_dds(&rgba, Some(&resident), Some(&streamed)).unwrap();
        assert_eq!(&dds[84..88], &[0; 4]);
        assert_eq!(dds.len(), 128 + 64 * 64 * 4);
        let bc3 = selected_header(256, 256, 8, 1, 3);
        let resident = mip_chain(128, 128, 8, 3);
        let dds = texture_export_selected_dds(&bc3, Some(&resident), None).unwrap();
        assert_eq!(&dds[84..88], b"DXT5");
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 128);
        assert_eq!(dds.len(), 128 + 128 * 128);
    }

    #[test]
    fn image_type_extension_detection() {
        assert_eq!(
            ImageType::from_extension("/SF/Interface/Foo.PNG"),
            ImageType::Png
        );
        assert_eq!(ImageType::from_extension("foo.jpg"), ImageType::Jpg);
        assert_eq!(ImageType::from_extension("foo.JPEG"), ImageType::Jpg);
        assert_eq!(ImageType::from_extension("foo.dds"), ImageType::Dds);
        assert_eq!(ImageType::from_extension("foo.bin"), ImageType::Unknown);
    }

    #[test]
    fn is_image_file_works() {
        assert!(is_image_file("a/b/c.PNG"));
        assert!(is_image_file("x.jpeg"));
        assert!(is_image_file("SF/Levels/0_cross.HDR"));
        assert!(is_image_file("SF/Levels/0_gbdcm.exr"));
        assert!(!is_image_file("x.bin"));
    }

    #[test]
    fn malformed_texture_headers_fail_without_panicking_or_unbounded_allocation() {
        for length in 0..128 {
            let mut bytes = vec![0; length];
            for offset in [16, 20, 24, 32] {
                if offset + 4 <= length {
                    bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                }
            }
            assert!(std::panic::catch_unwind(|| texture_unpack_header(&bytes))
                .unwrap()
                .is_err());
        }
        // A huge unknown-table count used to overflow num_unk * 4 in debug
        // builds and wrap in release builds before reading unrelated data.
        let mut bytes = vec![0; 40];
        bytes[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(texture_unpack_header(&bytes)
            .unwrap_err()
            .to_string()
            .contains("Truncated texture table"));
    }

    #[test]
    fn morton_known_values() {
        // (0,0)=0, (1,0)=1, (0,1)=2, (1,1)=3, (2,0)=4, (3,3)=15
        assert_eq!(morton_encode(0, 0), 0);
        assert_eq!(morton_encode(1, 0), 1);
        assert_eq!(morton_encode(0, 1), 2);
        assert_eq!(morton_encode(1, 1), 3);
        assert_eq!(morton_encode(2, 0), 4);
        assert_eq!(morton_encode(3, 3), 15);
    }
}
