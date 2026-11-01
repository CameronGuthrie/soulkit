//! Soulframe mesh decoding and glTF binary (`.glb`) export.
//!
//! A mesh lives in three cache entries with the same path: `H.Misc` holds the
//! header (layout, counts, mesh parts), `B.Misc` the low-detail vertex and
//! index data, and `F.Misc` (when installed) the remaining high-detail data.
//! Cache / TOC / Oodle plumbing lives in [`soulframe_cache`].
//!
//! Layouts follow the community format notes in Puxtril's Warframe-Exporter
//! (Soulframe shares the Evolution engine formats).

pub mod cli;

use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Value};
use soulframe_cache::CommonHeader;

/// Static mesh header type handled by [`decode_static`].
pub const TYPE_STATIC: u32 = 106;

/// A decoded mesh ready for export. Indices address the shared vertex arrays.
#[derive(Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
    pub parts: Vec<Part>,
}

/// One sub-mesh: a material slot drawing `count` indices from `first`.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub material: String,
    pub first: usize,
    pub count: usize,
    /// Level of detail exported (0 = highest detail).
    pub lod: usize,
}

/// Whether an `.fbx` path names a static mesh. In the installed cache the
/// name suffix identifies the header type exactly: `_anim` (134), `_cin`
/// (136), `_skel` (288), `_c` (113), `_sm` (109) and `_hlod`/`_dcm` (112);
/// unsuffixed meshes are static (106). Export still verifies the type.
pub fn is_static_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let Some(stem) = name.strip_suffix(".fbx") else {
        return false;
    };
    !["_anim", "_cin", "_skel", "_c", "_sm", "_dcm", "_hlod"]
        .iter()
        .any(|s| stem.ends_with(s))
}

struct Reader<'a> {
    bin: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bin: &'a [u8], pos: usize) -> Self {
        Self { bin, pos }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.bin.len())
            .ok_or_else(|| anyhow!("Truncated data at offset {}", self.pos))?;
        let bytes = &self.bin[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, len: usize) -> Result<()> {
        self.take(len).map(|_| ())
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn vec4(&mut self) -> Result<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }

    fn u32_max(&mut self, max: u32, what: &str) -> Result<u32> {
        let value = self.u32()?;
        ensure!(value <= max, "{what} out of range: {value} > {max}");
        Ok(value)
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    fn remaining(&self) -> usize {
        self.bin.len() - self.pos
    }
}

/// A physics descriptor: two type words, an optional pad and a path.
fn skip_physics(r: &mut Reader) -> Result<()> {
    let kind = r.u32()?;
    let sub = r.u32()?;
    if kind == 8 || sub == 8 {
        r.skip(2)?;
    }
    let len = r.u32()? as usize;
    r.skip(len)
}

struct MeshInfo {
    name: String,
    lod_offsets: [u32; 5],
    lod_counts: [u32; 5],
}

struct StaticHeader {
    scale: [f32; 3],
    vertex_count: usize,
    index_count: usize,
    vertex_count_b: usize,
    index_count_b: usize,
    infos: Vec<MeshInfo>,
    physx_bytes: usize,
}

fn read_static_header(bin: &[u8], start: usize) -> Result<StaticHeader> {
    let mut r = Reader::new(bin, start);
    r.skip(0x30)?;
    skip_physics(&mut r)?;
    r.skip(0x28)?;
    let a = r.vec4()?;
    let b = r.vec4()?;
    let vertex_count = r.u32()?;
    ensure!(
        // u16 indices; glTF also reserves 65535 for primitive restart.
        (1..=65_535).contains(&vertex_count),
        "Vertex count out of range: {vertex_count}"
    );
    let index_count = r.u32()?;
    let morphs = r.u32()?;
    ensure!(morphs == 0, "Morph targets are not supported yet");
    let _bones = r.u32()?;
    r.skip(0x22)?;
    let vertex_count_b = r.u32_max(vertex_count, "B vertex count")?;
    let index_count_b = r.u32_max(index_count, "B index count")?;
    r.skip(0x3D)?;

    let count = r.u32_max(50, "Mesh part count")?;
    let mut infos = Vec::with_capacity(count as usize);
    for _ in 0..count {
        r.skip(32)?; // two bounding vectors
        let name = r.string()?;
        let mut lod_offsets = [0; 5];
        let mut lod_counts = [0; 5];
        for value in &mut lod_offsets {
            *value = r.u32()?;
        }
        for value in &mut lod_counts {
            *value = r.u32()?;
        }
        r.skip(0x20)?;
        let unknown = r.u32()? as usize;
        r.skip(unknown + 0x10)?;
        infos.push(MeshInfo {
            name,
            lod_offsets,
            lod_counts,
        });
    }

    let shorts = r.u32_max(30, "Unknown u16 array length")?;
    r.skip(shorts as usize * 2)?;
    skip_physics(&mut r)?;

    // Collision meshes are stored ahead of the vertices in B.
    let physx = r.u32_max(300, "PhysX mesh count")?;
    let mut physx_bytes = 0usize;
    for _ in 0..physx {
        let kind = r.u32()?;
        r.skip(match kind {
            1 => 0x4C,
            7 => 0x60,
            _ => 0x50,
        })?;
        r.skip(32)?; // bounds
        if !matches!(kind, 0 | 2 | 3) {
            r.skip(4)?;
        }
        r.skip(4)?;
        physx_bytes = physx_bytes.saturating_add(r.u32()? as usize);
        r.skip(8)?;
    }

    let errors = r.u32_max(1000, "Error message count")?;
    for _ in 0..errors {
        r.string()?;
    }
    ensure!(
        r.remaining() == 0,
        "Static mesh header has {} unread bytes",
        r.remaining()
    );

    // Unused axes store 0; keep them unscaled.
    let scale = [0, 1, 2].map(|i| {
        let s = a[i].abs().max(b[i].abs());
        if s == 0.0 {
            1.0
        } else {
            s
        }
    });
    Ok(StaticHeader {
        scale,
        vertex_count: vertex_count as usize,
        index_count: index_count as usize,
        vertex_count_b: vertex_count_b as usize,
        index_count_b: index_count_b as usize,
        infos,
        physx_bytes,
    })
}

/// IEEE 754 half → single precision.
fn f16(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (h >> 10) & 0x1F;
    let man = f32::from(h & 0x3FF);
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 if man == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + man / 1024.0) * 2f32.powi(i32::from(exp) - 15),
    }
}

const VERTEX_STRIDE: usize = 24;

fn read_vertices(r: &mut Reader, count: usize, scale: [f32; 3], mesh: &mut Mesh) -> Result<()> {
    ensure!(
        r.remaining() / VERTEX_STRIDE >= count,
        "Body holds fewer than {count} vertices"
    );
    for _ in 0..count {
        let v = r.take(VERTEX_STRIDE)?;
        let i16_at = |o: usize| f32::from(i16::from_le_bytes([v[o], v[o + 1]])) / 32767.0;
        // A few source UVs overflow half precision (Inf/NaN); zero those
        // components rather than dropping the mesh.
        let h_at = |o: usize| match f16(u16::from_le_bytes([v[o], v[o + 1]])) {
            uv if uv.is_finite() => uv,
            _ => 0.0,
        };
        // The engine is left-handed; mirroring Z gives glTF's right-handed space.
        mesh.positions.push([
            i16_at(0) * scale[0],
            i16_at(2) * scale[1],
            -i16_at(4) * scale[2],
        ]);
        // v[8..16] holds normal, tangent and baked AO in an encoding that is
        // not decoded yet; glTF importers compute normals when absent.
        mesh.uv0.push([h_at(16), h_at(18)]);
        mesh.uv1.push([h_at(20), h_at(22)]);
    }
    Ok(())
}

fn read_indices(r: &mut Reader, count: usize, out: &mut Vec<u16>) -> Result<()> {
    ensure!(
        r.remaining() / 2 >= count,
        "Body holds fewer than {count} indices"
    );
    for _ in 0..count {
        out.push(r.u16()?);
    }
    Ok(())
}

/// Material names from a header's `Materials={ ... }` attribute block.
/// Relative names are resolved against the mesh's folder.
fn material_names(attributes: &[u8], mesh_path: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(attributes);
    let Some(start) = text.find("Materials={") else {
        return Vec::new();
    };
    let body = &text[start + "Materials={".len()..];
    let body = &body[..body.find('}').unwrap_or(body.len())];
    let folder = mesh_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    body.split([',', '\n'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| match name.starts_with('/') || folder.is_empty() {
            true => name.to_owned(),
            false => format!("/{}/{name}", folder.trim_start_matches('/')),
        })
        .collect()
}

/// Decode a static mesh (type 106) from its H, B and optional F entries.
pub fn decode_static(
    path: &str,
    header: &CommonHeader,
    h: &[u8],
    b: &[u8],
    f: Option<&[u8]>,
) -> Result<Mesh> {
    ensure!(
        header.type_id == TYPE_STATIC,
        "Not a static mesh (type {})",
        header.type_id
    );
    let hdr = read_static_header(h, header.end)?;
    ensure!(!hdr.infos.is_empty(), "Mesh has no parts");
    ensure!(!b.is_empty(), "Mesh has no body");

    // An empty B split means the whole mesh is stored in B.
    let (vertices_b, indices_b) = match hdr.vertex_count_b {
        0 => (hdr.vertex_count, hdr.index_count),
        n => (n, hdr.index_count_b),
    };
    let in_f = vertices_b < hdr.vertex_count || indices_b < hdr.index_count;
    let f = f.filter(|f| !f.is_empty() && in_f);

    let mut mesh = Mesh::default();
    let mut rb = Reader::new(b, 0);
    rb.skip(hdr.physx_bytes)?;
    rb.skip(2)?;
    read_vertices(&mut rb, vertices_b, hdr.scale, &mut mesh)?;
    read_indices(&mut rb, indices_b, &mut mesh.indices)?;
    if let Some(f) = f {
        let mut rf = Reader::new(f, 0);
        read_vertices(&mut rf, hdr.vertex_count - vertices_b, hdr.scale, &mut mesh)?;
        read_indices(&mut rf, hdr.index_count - indices_b, &mut mesh.indices)?;
    }

    // Use each part's most detailed LOD whose indices and vertices are present.
    let vertices = mesh.positions.len();
    let materials = material_names(&header.attributes, path);
    for (i, info) in hdr.infos.iter().enumerate() {
        let lod = (0..5).find(|&l| {
            let (first, count) = (info.lod_offsets[l] as usize, info.lod_counts[l] as usize);
            count > 0
                && count.is_multiple_of(3)
                && first + count <= mesh.indices.len()
                && mesh.indices[first..first + count]
                    .iter()
                    .all(|&i| (i as usize) < vertices)
        });
        let Some(lod) = lod else {
            continue;
        };
        mesh.parts.push(Part {
            name: info.name.clone(),
            material: materials.get(i).cloned().unwrap_or_default(),
            first: info.lod_offsets[lod] as usize,
            count: info.lod_counts[lod] as usize,
            lod,
        });
    }
    if mesh.parts.is_empty() {
        bail!("No mesh part has a complete level of detail");
    }
    Ok(mesh)
}

fn pad4(buf: &mut Vec<u8>, byte: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(byte);
    }
}

/// Serialize a mesh as a self-contained glTF 2.0 binary.
pub fn to_glb(mesh: &Mesh, name: &str) -> Result<Vec<u8>> {
    let n = mesh.positions.len();
    ensure!(n > 0 && !mesh.parts.is_empty(), "Empty mesh");
    ensure!(
        mesh.uv0.len() == n && mesh.uv1.len() == n,
        "Vertex attribute counts differ"
    );

    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut add_view = |bin: &mut Vec<u8>, bytes: &[u8], target: u32| {
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        pad4(bin, 0);
        views.push(
            json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len(), "target": target}),
        );
        views.len() - 1
    };
    let floats = |data: &mut dyn Iterator<Item = f32>| -> Vec<u8> {
        data.flat_map(f32::to_le_bytes).collect()
    };

    const ARRAY_BUFFER: u32 = 34962;
    const ELEMENT_ARRAY_BUFFER: u32 = 34963;
    let pos = add_view(
        &mut bin,
        &floats(&mut mesh.positions.iter().flatten().copied()),
        ARRAY_BUFFER,
    );
    let uv0 = add_view(
        &mut bin,
        &floats(&mut mesh.uv0.iter().flatten().copied()),
        ARRAY_BUFFER,
    );
    let uv1 = add_view(
        &mut bin,
        &floats(&mut mesh.uv1.iter().flatten().copied()),
        ARRAY_BUFFER,
    );
    // Each part's triangles, packed back to back. Mirroring Z flipped the
    // handedness, so swap two corners to keep glTF's counter-clockwise fronts.
    let mut idx_bytes = Vec::new();
    for part in &mesh.parts {
        ensure!(
            part.count.is_multiple_of(3),
            "Part {} is not a triangle list",
            part.name
        );
        let tris = mesh
            .indices
            .get(part.first..part.first + part.count)
            .ok_or_else(|| anyhow!("Part {} indices out of range", part.name))?;
        for t in tris.chunks_exact(3) {
            ensure!(t.iter().all(|&i| (i as usize) < n), "Index out of range");
            for i in [t[0], t[2], t[1]] {
                idx_bytes.extend(i.to_le_bytes());
            }
        }
    }
    let idx = add_view(&mut bin, &idx_bytes, ELEMENT_ARRAY_BUFFER);

    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in &mesh.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    ensure!(
        // min/max skip NaN, so check every component.
        mesh.positions
            .iter()
            .flatten()
            .chain(mesh.uv0.iter().flatten())
            .chain(mesh.uv1.iter().flatten())
            .all(|v| v.is_finite()),
        "Non-finite vertex data"
    );

    const FLOAT: u32 = 5126;
    const UNSIGNED_SHORT: u32 = 5123;
    let mut accessors = vec![
        json!({"bufferView": pos, "componentType": FLOAT, "count": n, "type": "VEC3", "min": min, "max": max}),
        json!({"bufferView": uv0, "componentType": FLOAT, "count": n, "type": "VEC2"}),
        json!({"bufferView": uv1, "componentType": FLOAT, "count": n, "type": "VEC2"}),
    ];

    let mut materials: Vec<String> = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    let mut offset = 0;
    for part in &mesh.parts {
        accessors.push(json!({
            "bufferView": idx, "byteOffset": offset * 2, "componentType": UNSIGNED_SHORT,
            "count": part.count, "type": "SCALAR",
        }));
        offset += part.count;
        let mut primitive = json!({
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1, "TEXCOORD_1": 2},
            "indices": accessors.len() - 1,
            "mode": 4,
        });
        if !part.material.is_empty() {
            let slot = match materials.iter().position(|m| m == &part.material) {
                Some(slot) => slot,
                None => {
                    materials.push(part.material.clone());
                    materials.len() - 1
                }
            };
            primitive["material"] = json!(slot);
        }
        meshes.push(json!({"name": part.name, "primitives": [primitive]}));
        nodes.push(
            json!({"name": part.name, "mesh": meshes.len() - 1, "extras": {"lod": part.lod}}),
        );
    }

    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "Soulkit"},
        "scene": 0,
        "scenes": [{"name": name, "nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    if !materials.is_empty() {
        doc["materials"] = Value::from(
            materials
                .iter()
                .map(|m| json!({"name": m, "doubleSided": false}))
                .collect::<Vec<_>>(),
        );
    }

    let mut json_bytes = serde_json::to_vec(&doc)?;
    pad4(&mut json_bytes, b' ');
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend(2u32.to_le_bytes());
    glb.extend(u32::try_from(total)?.to_le_bytes());
    glb.extend(u32::try_from(json_bytes.len())?.to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend(json_bytes);
    glb.extend(u32::try_from(bin.len())?.to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend(bin);
    Ok(glb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16(0x0000), 0.0);
        assert_eq!(f16(0x3C00), 1.0);
        assert_eq!(f16(0xC000), -2.0);
        assert_eq!(f16(0x3800), 0.5);
        assert_eq!(f16(0x0001), 2f32.powi(-24));
        assert!(f16(0x7C00).is_infinite());
    }

    #[test]
    fn material_block_resolves_relative_names() {
        let attrs = b"Foo=1\nMaterials={\n    Stone,\n    /SF/Shared/Moss\n}\n";
        assert_eq!(
            material_names(attrs, "SF/Objects/Rock.fbx"),
            ["/SF/Objects/Stone", "/SF/Shared/Moss"]
        );
        assert!(material_names(b"", "a.fbx").is_empty());
    }

    #[test]
    fn suffixed_names_are_not_static_meshes() {
        assert!(is_static_name("SF/Objects/Rock.fbx"));
        assert!(is_static_name("SF/Objects/Rock_Cliff.FBX"));
        assert!(!is_static_name("SF/Levels/A/0_sm.fbx"));
        assert!(!is_static_name("SF/Objects/Tree_DCM.fbx"));
        assert!(!is_static_name("SF/Animations/Run_anim.fbx"));
        assert!(!is_static_name("SF/Levels/A/0_c.fbx"));
        assert!(!is_static_name("SF/Sounds/a.wav"));
    }

    /// Build H and B for one triangle with the documented static layout.
    fn triangle() -> (Vec<u8>, Vec<u8>) {
        let mut h = vec![0u8; 16];
        h.extend(0u32.to_le_bytes()); // no source paths
        let attrs = b"Materials={\n    Mat\n}";
        h.extend((attrs.len() as u32).to_le_bytes());
        h.extend(attrs);
        h.push(0);
        h.extend(TYPE_STATIC.to_le_bytes());
        let physics = |h: &mut Vec<u8>| h.extend([0u8; 12]);
        h.extend([0u8; 0x30]);
        physics(&mut h);
        h.extend([0u8; 0x28]);
        for v in [2.0f32, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0] {
            h.extend(v.to_le_bytes());
        }
        for v in [3u32, 3, 0, 0] {
            h.extend(v.to_le_bytes()); // vertices, indices, morphs, bones
        }
        h.extend([0u8; 0x22]);
        h.extend([0u8; 8]); // B split 0 = everything in B
        h.extend([0u8; 0x3D]);
        h.extend(1u32.to_le_bytes());
        h.extend([0u8; 32]);
        h.extend(4u32.to_le_bytes());
        h.extend(b"Body");
        for v in [0u32, 0, 0, 0, 0, 3, 0, 0, 0, 0] {
            h.extend(v.to_le_bytes());
        }
        h.extend([0u8; 0x20]);
        h.extend(0u32.to_le_bytes());
        h.extend([0u8; 0x10]);
        h.extend(0u32.to_le_bytes()); // u16 array
        physics(&mut h);
        h.extend(0u32.to_le_bytes()); // physx meshes
        h.extend(0u32.to_le_bytes()); // errors

        let mut b = vec![0u8; 2];
        for (x, y, z) in [(32767i16, 0i16, 0i16), (0, 32767, 0), (0, 0, 32767)] {
            for c in [x, y, z, 0] {
                b.extend(c.to_le_bytes());
            }
            b.extend([0u8; 8]); // normal, tangent, AO
            b.extend([0x00, 0x3C, 0x00, 0x00, 0, 0, 0, 0]); // uv0 = (1, 0)
        }
        for i in [0u16, 1, 2] {
            b.extend(i.to_le_bytes());
        }
        (h, b)
    }

    #[test]
    fn decodes_a_static_triangle_and_writes_a_valid_glb() {
        let (h, b) = triangle();
        let header = soulframe_cache::parse_common_header(&h).unwrap();
        let mesh = decode_static("SF/Objects/Tri.fbx", &header, &h, &b, None).unwrap();
        assert_eq!(mesh.positions[0], [2.0, 0.0, -0.0]);
        assert_eq!(mesh.positions[2], [0.0, 0.0, -1.0]); // Z mirrored
        assert_eq!(mesh.uv0[1], [1.0, 0.0]);
        assert_eq!(mesh.parts.len(), 1);
        assert_eq!(mesh.parts[0].material, "/SF/Objects/Mat");

        let glb = to_glb(&mesh, "Tri").unwrap();
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        assert_eq!(doc["accessors"][3]["count"], 3);
        // Winding is reversed to compensate for the Z mirror.
        let bin = &glb[20 + json_len + 8..];
        let view = &doc["bufferViews"][3];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        assert_eq!(&bin[start..start + 6], &[0, 0, 2, 0, 1, 0]);
        assert_eq!(doc["materials"][0]["name"], "/SF/Objects/Mat");
    }

    #[test]
    fn non_finite_vertex_data_is_rejected() {
        let (h, b) = triangle();
        let header = soulframe_cache::parse_common_header(&h).unwrap();
        let mut mesh = decode_static("a.fbx", &header, &h, &b, None).unwrap();
        mesh.uv1[2][0] = f32::NAN;
        assert!(to_glb(&mesh, "a").is_err());
    }

    #[test]
    fn truncated_bodies_and_headers_fail_cleanly() {
        let (h, b) = triangle();
        let header = soulframe_cache::parse_common_header(&h).unwrap();
        for cut in [0, 10, b.len() - 1] {
            assert!(decode_static("a.fbx", &header, &h, &b[..cut], None).is_err());
        }
        assert!(decode_static("a.fbx", &header, &h[..h.len() - 1], &b, None).is_err());
    }
}
