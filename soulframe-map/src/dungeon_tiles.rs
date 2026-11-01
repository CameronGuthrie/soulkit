//! Procedural dungeon room floor plans rendered from navmeshes
//! (`SF/Levels/Procs/<Set>/<Room>/0.nav`). Each set is written like a tiled level map:
//! `<setKey>/{n}.webp` (rooms numbered in name order) plus `<setKey>/rooms.json`.
//!
//! The rooms' `*_mm.png` LightMaps are the engine's coarse minimap capture
//! (8-256 px, blurred), so the floor plan is drawn from the walkable navmesh
//! instead. H.Misc holds a header with the mesh counts; the B.Misc body starts
//! with those arrays back to back:
//!
//! | block      | record                                                   |
//! |------------|----------------------------------------------------------|
//! | vertices   | `f32 x, y, z` (y up)                                     |
//! | edges      | `u16 v0, v1, tri_a, tri_b, 0xFFFF` (`tri_b` = FFFF: wall) |
//! | triangles  | `u16 edge0, edge1, edge2`                                |
//!
//! Further navigation data follows; it is not needed for drawing.
use crate::cli::{level_key, CacheStore};
use anyhow::{anyhow, bail, ensure, Context, Result};
use image::{Rgb, RgbImage, Rgba, RgbaImage};
use serde_json::json;
use soulframe_cache::Oodle;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::Path;

const PROCS_PREFIX: &str = "SF/Levels/Procs/";
const NAV_FILE: &str = "0.nav";
const NO_TRIANGLE: u16 = 0xFFFF;

/// Pixels per metre, reduced for rooms that would exceed `MAX_IMAGE_SIDE`.
const PIXELS_PER_METRE: f32 = 12.0;
const MAX_IMAGE_SIDE: f32 = 4096.0;
/// Empty border around the navmesh bounds, in metres.
const PADDING_METRES: f32 = 3.0;
const GRID_METRES: f32 = 5.0;
/// Wall outlines are rounded by Chaikin corner cutting, cutting at most this
/// far into each segment per pass so long straight walls keep crisp lengths.
const SMOOTH_CUT_METRES: f32 = 1.0;
const SMOOTH_PASSES: usize = 4;
const WALL_WIDTH_PX: f32 = 2.2;
/// Walls this far below the drawn (topmost) floor belong to a hidden storey.
const HIDDEN_WALL_METRES: f32 = 1.5;

const BACKGROUND: [f32; 3] = [17.0, 19.0, 25.0];
const GRID: [f32; 3] = [30.0, 34.0, 44.0];
const WALL: [f32; 3] = [242.0, 240.0, 234.0];
/// Height ramp from the room's lowest (0) to highest (1) walkable point.
const HEIGHT_RAMP: &[(f32, [f32; 3])] = &[
    (0.0, [58.0, 74.0, 112.0]),
    (0.45, [120.0, 136.0, 156.0]),
    (0.75, [196.0, 184.0, 150.0]),
    (1.0, [236.0, 214.0, 160.0]),
];

/// Per-set room name prefixes, stripped before reading the room kind.
const SET_PREFIXES: &[&str] = &["UnderCity", "Crypt", "Mines", "GoAx", "NEAx", "FFx", "OSCx"];
/// Room-kind tokens that follow the set prefix, longest first where they overlap.
const ROOM_KINDS: &[(&str, &str)] = &[
    ("SpecialCheckpoint", "checkpoint"),
    ("Streaming", "streaming"),
    ("Entrance", "entrance"),
    ("DeadEnd", "dead_end"),
    ("CaveCap", "cap"),
    ("Fitting", "fitting"),
    ("Spawn", "spawn"),
    ("Exit", "exit"),
    ("Boss", "boss"),
    ("Con", "connector"),
    ("Int", "interior"),
    ("Cap", "cap"),
    ("Obj", "objective"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoomRef {
    set: String,
    room: String,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NavCounts {
    vertices: usize,
    edges: usize,
    triangles: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NavEdge {
    v0: u16,
    v1: u16,
    tri_a: u16,
    tri_b: u16,
}

impl NavEdge {
    fn is_wall(&self) -> bool {
        self.tri_a == NO_TRIANGLE || self.tri_b == NO_TRIANGLE
    }
}

#[derive(Debug, Clone)]
struct NavMesh {
    vertices: Vec<[f32; 3]>,
    edges: Vec<NavEdge>,
    triangles: Vec<[u16; 3]>,
}

/// Maps room-local metres to image pixels: x right, z up (north up).
#[derive(Debug, Clone, Copy)]
struct Projection {
    pixels_per_metre: f32,
    left: f32,
    top: f32,
}

impl Projection {
    fn pixel(&self, [x, _, z]: [f32; 3]) -> (f32, f32) {
        (
            (x - self.left) * self.pixels_per_metre,
            (self.top - z) * self.pixels_per_metre,
        )
    }
}

struct FloorPlan {
    image: RgbImage,
    outline_image: RgbaImage,
    outline: Vec<serde_json::Value>,
    projection: Projection,
    min: [f32; 3],
    max: [f32; 3],
}

pub(crate) fn extract_dungeon_tiles(
    cache_path: &Path,
    oodle: &Oodle,
    out_dir: &Path,
) -> Result<usize> {
    println!("Scanning procedural dungeon room navmeshes...");
    let caches = CacheStore::load_named(cache_path, &["H.Misc", "B.Misc"])?;
    let levels = CacheStore::load_named(cache_path, &["H.AnimRetarget"])?;
    let level_paths: std::collections::BTreeSet<&str> = levels
        .entries()
        .into_iter()
        .filter(|e| !e.entry.is_dir)
        .map(|e| e.entry.full_path.as_str())
        .collect();

    let rooms = discover_rooms(
        caches
            .entries()
            .into_iter()
            .filter(|e| e.cache_name.starts_with('H') && !e.entry.is_dir)
            .map(|e| e.entry.full_path.as_str()),
    );
    println!("Found {} room navmesh(es)", rooms.len());
    // Per set key: (room records, skipped rooms).
    let mut sets = BTreeMap::<String, (Vec<serde_json::Value>, Vec<serde_json::Value>)>::new();
    for room in &rooms {
        let (records, skipped) = sets.entry(level_key(&room.set)).or_default();
        let mesh = match read_nav(&caches, oodle, &room.path) {
            Ok(mesh) => mesh,
            Err(e) => {
                eprintln!("  [dungeon] skipped {}: {e:#}", room.path);
                skipped.push(json!({"path": room.path, "error": format!("{e:#}")}));
                continue;
            }
        };
        let plan = render_floor_plan(&mesh);
        // Rooms arrive sorted by name, so `n` is the room's position within its set.
        let file = format!("{}.webp", records.len());
        let outline_file = format!("{}.outline.webp", records.len());
        let set_dir = out_dir.join(level_key(&room.set));
        let out_path = set_dir.join(&file);
        fs::create_dir_all(&set_dir)
            .with_context(|| format!("Failed to create {}", set_dir.display()))?;
        let mut out_file = File::create(&out_path)
            .with_context(|| format!("Failed to create {}", out_path.display()))?;
        image::codecs::webp::WebPEncoder::new_lossless(&mut out_file)
            .encode(
                plan.image.as_raw(),
                plan.image.width(),
                plan.image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| anyhow!("Failed to save {}: {e}", out_path.display()))?;
        let outline_path = set_dir.join(&outline_file);
        let mut outline_output = File::create(&outline_path)?;
        image::codecs::webp::WebPEncoder::new_lossless(&mut outline_output)
            .encode(plan.outline_image.as_raw(), plan.outline_image.width(), plan.outline_image.height(), image::ExtendedColorType::Rgba8)
            .with_context(|| format!("Failed to save {}", outline_path.display()))?;

        let level = format!("{PROCS_PREFIX}{}/{}.level", room.set, room.room);
        let p = plan.projection;
        records.push(json!({
            "set": room.set,
            "room": room.room,
            "kind": room_kind(&room.room),
            "file": file,
            "outline_file": outline_file,
            "outline": plan.outline,
            "width": plan.image.width(),
            "height": plan.image.height(),
            "pixels_per_metre": p.pixels_per_metre,
            "image_origin": {"x": p.left, "z": p.top},
            "bounds": {
                "x": [plan.min[0], plan.max[0]],
                "y": [plan.min[1], plan.max[1]],
                "z": [plan.min[2], plan.max[2]],
            },
            "vertices": mesh.vertices.len(),
            "triangles": mesh.triangles.len(),
            "wall_edges": mesh.edges.iter().filter(|e| e.is_wall()).count(),
            "source_path": room.path,
            "level_path": level_paths.contains(level.as_str()).then_some(level),
        }));
    }

    let mut written = 0;
    let mut skipped_total = 0;
    for (key, (records, skipped)) in sets {
        written += records.len();
        skipped_total += skipped.len();
        write_rooms_json(&out_dir.join(&key), records, skipped)?;
    }
    println!("Dungeon tiles complete: {written} written, {skipped_total} skipped");
    Ok(written)
}

fn write_rooms_json(set_dir: &Path, records: Vec<serde_json::Value>, skipped: Vec<serde_json::Value>) -> Result<()> {
    let written = records.len();
    let document = json!({
        "schema": 2,
        "source": "B.Misc SF/Levels/Procs/<Set>/<Room>/0.nav (counts from the H.Misc header)",
        "rendering": {
            "content": "walkable navmesh only, topmost surface where floors overlap",
            "orientation": "room-local metres; +x right, +z up (north up); world placement not decoded",
            "pixel_from_metres": "px = (x - image_origin.x) * pixels_per_metre, py = (image_origin.z - z) * pixels_per_metre",
            "fill": "height ramp from bounds.y[0] (slate blue) to bounds.y[1] (sand), with hillshade",
            "outline": "navmesh boundary loops rounded by Chaikin corner cutting; floor clipped to the rounded outline",
            "outline_coordinates": "room-local x/z metres; separate storeys projected into one plan",
            "outline_file": "white walls on a transparent RGBA background; no baked floor or grid",
            "smoothing_cut_metres": SMOOTH_CUT_METRES,
            "smoothing_passes": SMOOTH_PASSES,
            "grid_metres": GRID_METRES,
            "walls": "rounded navmesh boundary edges",
        },
        "kind_rule": "room-name token after the set prefix; null when no token matches",
        "count": written,
        "skipped": skipped,
        "rooms": records,
    });
    fs::create_dir_all(set_dir)?;
    let json_path = set_dir.join("rooms.json");
    fs::write(&json_path, serde_json::to_string(&document)? + "
")
        .with_context(|| format!("Failed to write {}", json_path.display()))
}

fn discover_rooms<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<RoomRef> {
    let mut rooms: Vec<RoomRef> = paths
        .filter_map(|path| {
            let rest = path.strip_prefix(PROCS_PREFIX)?;
            let mut parts = rest.split('/');
            let (set, room, file) = (parts.next()?, parts.next()?, parts.next()?);
            (file == NAV_FILE && parts.next().is_none() && !set.is_empty() && !room.is_empty())
                .then(|| RoomRef {
                    set: set.to_string(),
                    room: room.to_string(),
                    path: path.to_string(),
                })
        })
        .collect();
    rooms.sort_by(|a, b| a.path.cmp(&b.path));
    rooms.dedup();
    rooms
}

fn room_kind(room: &str) -> Option<&'static str> {
    let rest = SET_PREFIXES
        .iter()
        .find_map(|prefix| room.strip_prefix(prefix))
        .unwrap_or(room);
    ROOM_KINDS
        .iter()
        .find(|(token, _)| {
            rest.get(..token.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(token))
        })
        .map(|&(_, kind)| kind)
}

/// Reads the newest header and the newest body it describes.
fn read_nav(caches: &CacheStore, oodle: &Oodle, path: &str) -> Result<NavMesh> {
    let mut entries = caches.matching_entries(path);
    entries.sort_by_key(|e| (Reverse(e.entry.timestamp), Reverse(e.entry.index)));
    let header = entries
        .iter()
        .find(|e| e.cache_name.starts_with('H'))
        .context("no H.Misc header")?;
    let counts = nav_counts(&caches.read_entry(oodle, header.cache_index, header.entry)?)?;
    let mut last_error = anyhow!("no B.Misc body for {path}");
    for entry in entries.iter().filter(|e| e.cache_name.starts_with('B')) {
        let body = caches.read_entry(oodle, entry.cache_index, entry.entry)?;
        match parse_nav(counts, &body) {
            Ok(mesh) => return Ok(mesh),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Header: 16-byte hash, dependency paths, a reserved u32, then a sized
/// payload of `hash, vertex count, edge count, ?, ?, ?, triangle count, ...`.
fn nav_counts(header: &[u8]) -> Result<NavCounts> {
    let mut reader = Reader { bytes: header, pos: 16 };
    let dependencies = reader.u32()?;
    for _ in 0..dependencies {
        let len = reader.u32()? as usize;
        reader.skip(len)?;
    }
    reader.skip(4)?;
    let payload_len = reader.u32()? as usize;
    ensure!(payload_len >= 28, "nav header payload too short ({payload_len} bytes)");
    reader.skip(4)?; // hash
    let vertices = reader.u32()? as usize;
    let edges = reader.u32()? as usize;
    reader.skip(12)?;
    let triangles = reader.u32()? as usize;
    ensure!(vertices >= 3 && triangles >= 1, "empty navmesh");
    ensure!(
        vertices <= usize::from(u16::MAX) && triangles < usize::from(NO_TRIANGLE),
        "navmesh counts exceed u16 indices"
    );
    Ok(NavCounts { vertices, edges, triangles })
}

fn parse_nav(counts: NavCounts, body: &[u8]) -> Result<NavMesh> {
    let needed = counts.vertices * 12 + counts.edges * 10 + counts.triangles * 6;
    ensure!(
        body.len() >= needed,
        "nav body has {} bytes, header counts need {needed}",
        body.len()
    );
    let mut reader = Reader { bytes: body, pos: 0 };
    let vertices = (0..counts.vertices)
        .map(|_| Ok([reader.f32()?, reader.f32()?, reader.f32()?]))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        vertices.iter().flatten().all(|v| v.is_finite()),
        "non-finite navmesh vertex"
    );
    let mut edges = Vec::with_capacity(counts.edges);
    for index in 0..counts.edges {
        let edge = NavEdge {
            v0: reader.u16()?,
            v1: reader.u16()?,
            tri_a: reader.u16()?,
            tri_b: reader.u16()?,
        };
        ensure!(reader.u16()? == NO_TRIANGLE, "edge {index} has no FFFF terminator");
        ensure!(
            usize::from(edge.v0.max(edge.v1)) < counts.vertices,
            "edge {index} vertex out of range"
        );
        edges.push(edge);
    }
    let mut triangles = Vec::with_capacity(counts.triangles);
    for index in 0..counts.triangles {
        let mut corners: Vec<u16> = Vec::with_capacity(6);
        for _ in 0..3 {
            let edge = edges
                .get(usize::from(reader.u16()?))
                .with_context(|| format!("triangle {index} edge out of range"))?;
            ensure!(
                usize::from(edge.tri_a) == index || usize::from(edge.tri_b) == index,
                "triangle {index} lists an edge that does not border it"
            );
            corners.extend([edge.v0, edge.v1]);
        }
        corners.sort_unstable();
        corners.dedup();
        let &[a, b, c] = corners.as_slice() else {
            bail!("triangle {index} has {} distinct vertices", corners.len());
        };
        triangles.push([a, b, c]);
    }
    Ok(NavMesh { vertices, edges, triangles })
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self
            .bytes
            .get(self.pos..self.pos + N)
            .context("unexpected end of nav data")?;
        self.pos += N;
        Ok(bytes.try_into().unwrap())
    }

    fn skip(&mut self, len: usize) -> Result<()> {
        ensure!(self.pos + len <= self.bytes.len(), "unexpected end of nav data");
        self.pos += len;
        Ok(())
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
}

fn render_floor_plan(mesh: &NavMesh) -> FloorPlan {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }
    let span_x = max[0] - min[0] + 2.0 * PADDING_METRES;
    let span_z = max[2] - min[2] + 2.0 * PADDING_METRES;
    let pixels_per_metre = PIXELS_PER_METRE.min(MAX_IMAGE_SIDE / span_x.max(span_z));
    let projection = Projection {
        pixels_per_metre,
        left: min[0] - PADDING_METRES,
        top: max[2] + PADDING_METRES,
    };
    let width = (span_x * pixels_per_metre).ceil().max(1.0) as usize;
    let height = (span_z * pixels_per_metre).ceil().max(1.0) as usize;

    let chains: Vec<_> = wall_chains(mesh)
        .into_iter()
        .map(|(points, closed)| (round_corners(points, closed), closed))
        .collect();
    let outline = chains.iter().map(|(points, closed)| {
        let points: Vec<_> = simplify_outline(points, *closed, 0.12)
            .into_iter().map(|p| [p[0], p[2]]).collect();
        json!({"points": points, "closed": closed})
    }).collect();
    let walls: Vec<[[f32; 3]; 2]> = chains.into_iter()
        .flat_map(|(points, closed)| {
            let count = if closed { points.len() } else { points.len().saturating_sub(1) };
            (0..count).map(move |i| [points[i], points[(i + 1) % points.len()]])
        })
        .collect();
    let inside = rasterize_outline(&walls, projection, width, height);
    let mut heights = rasterize_heights(mesh, projection, width, height);
    clip_to_outline(&mut heights, &inside, width, height);
    let wall_cover = rasterize_walls(&walls, projection, &heights, width, height);
    let height_span = (max[1] - min[1]).max(f32::EPSILON);
    let floor_at = |x: usize, y: usize| Some(heights[y * width + x]).filter(|h| h.is_finite());

    let mut image = RgbImage::new(width as u32, height as u32);
    let mut outline_image = RgbaImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let mut color = match floor_at(x, y) {
                Some(h) => {
                    let mut c = ramp((h - min[1]) / height_span);
                    // Central differences over floor neighbours, in metres per pixel.
                    let sample = |dx: isize, dy: isize| {
                        let (nx, ny) = (x as isize + dx, y as isize + dy);
                        (nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height)
                            .then(|| floor_at(nx as usize, ny as usize))
                            .flatten()
                            .unwrap_or(h)
                    };
                    let gx = (sample(1, 0) - sample(-1, 0)) / 2.0;
                    let gy = (sample(0, 1) - sample(0, -1)) / 2.0;
                    let shade = (1.0 - (gx + gy) * 0.21).clamp(0.75, 1.2);
                    c.iter_mut().for_each(|channel| *channel *= shade);
                    c
                }
                None if on_grid(x, projection.left, 1.0, pixels_per_metre)
                    || on_grid(y, projection.top, -1.0, pixels_per_metre) =>
                {
                    GRID
                }
                None => BACKGROUND,
            };
            let cover = wall_cover[y * width + x];
            outline_image.put_pixel(x as u32, y as u32, Rgba([255, 255, 255, (cover * 255.0).round() as u8]));
            for (channel, wall) in color.iter_mut().zip(WALL) {
                *channel += (wall - *channel) * cover;
            }
            image.put_pixel(x as u32, y as u32, Rgb(color.map(|c| c.round().clamp(0.0, 255.0) as u8)));
        }
    }
    FloorPlan { image, outline_image, outline, projection, min, max }
}

/// Keep the rounded silhouette compact enough for a client to stroke directly.
fn simplify_outline(points: &[[f32; 3]], closed: bool, tolerance: f32) -> Vec<[f32; 3]> {
    fn simplify(points: &[[f32; 3]], tolerance: f32, result: &mut Vec<[f32; 3]>) {
        if points.len() < 3 { result.extend_from_slice(points); return; }
        let (a, b) = (points[0], points[points.len()-1]);
        let (dx, dz) = (b[0]-a[0], b[2]-a[2]);
        let length2 = dx*dx + dz*dz;
        let mut farthest = (0, 0.0f32);
        for (index, p) in points.iter().enumerate().skip(1).take(points.len()-2) {
            let t = if length2 > 0.0 { ((p[0]-a[0])*dx + (p[2]-a[2])*dz) / length2 } else { 0.0 }.clamp(0.0, 1.0);
            let distance = ((p[0]-a[0]-t*dx).powi(2) + (p[2]-a[2]-t*dz).powi(2)).sqrt();
            if distance > farthest.1 { farthest = (index, distance); }
        }
        if farthest.1 > tolerance {
            simplify(&points[..=farthest.0], tolerance, result);
            result.pop();
            simplify(&points[farthest.0..], tolerance, result);
        } else { result.extend([a, b]); }
    }
    if points.len() < 3 { return points.to_vec(); }
    let mut result = Vec::new();
    if closed {
        let halfway = points.len()/2;
        simplify(&points[..=halfway], tolerance, &mut result);
        result.pop();
        let tail: Vec<_> = points[halfway..].iter().copied().chain([points[0]]).collect();
        simplify(&tail, tolerance, &mut result);
        result.pop();
    } else { simplify(points, tolerance, &mut result); }
    result
}

/// Whether pixel column/row `index` holds a grid line. `direction` is +1 for x
/// (metres grow with pixels) and -1 for z (metres shrink as rows grow).
fn on_grid(index: usize, origin: f32, direction: f32, pixels_per_metre: f32) -> bool {
    let metres = origin + direction * (index as f32 + 0.5) / pixels_per_metre;
    let offset = metres.rem_euclid(GRID_METRES) * pixels_per_metre;
    offset < 0.5 || GRID_METRES * pixels_per_metre - offset <= 0.5
}

/// Topmost interpolated floor height per pixel centre; `-inf` outside the mesh.
fn rasterize_heights(mesh: &NavMesh, projection: Projection, width: usize, height: usize) -> Vec<f32> {
    let mut heights = vec![f32::NEG_INFINITY; width * height];
    for triangle in &mesh.triangles {
        let corners = triangle.map(|i| mesh.vertices[usize::from(i)]);
        let [(x0, y0), (x1, y1), (x2, y2)] = corners.map(|v| projection.pixel(v));
        let area = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
        if area.abs() < 1e-6 {
            continue;
        }
        let (min_x, max_x) = pixel_range(x0.min(x1).min(x2), x0.max(x1).max(x2), width);
        let (min_y, max_y) = pixel_range(y0.min(y1).min(y2), y0.max(y1).max(y2), height);
        for py in min_y..max_y {
            for px in min_x..max_x {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let l0 = ((y1 - y2) * (cx - x2) + (x2 - x1) * (cy - y2)) / area;
                let l1 = ((y2 - y0) * (cx - x2) + (x0 - x2) * (cy - y2)) / area;
                let l2 = 1.0 - l0 - l1;
                // Small tolerance closes hairline gaps between neighbours.
                if l0 < -1e-3 || l1 < -1e-3 || l2 < -1e-3 {
                    continue;
                }
                let h = l0 * corners[0][1] + l1 * corners[1][1] + l2 * corners[2][1];
                let slot = &mut heights[py * width + px];
                *slot = slot.max(h);
            }
        }
    }
    heights
}

/// Navmesh boundary edges chained into polylines, each oriented with its floor
/// triangle on the left (x right, z up). `true` marks a closed loop.
fn wall_chains(mesh: &NavMesh) -> Vec<(Vec<[f32; 3]>, bool)> {
    let directed: Vec<(u16, u16)> = mesh
        .edges
        .iter()
        .filter(|e| e.is_wall())
        .filter_map(|e| {
            let owner = if e.tri_a == NO_TRIANGLE { e.tri_b } else { e.tri_a };
            let triangle = mesh.triangles.get(usize::from(owner))?;
            let &third = triangle.iter().find(|&&v| v != e.v0 && v != e.v1)?;
            let [a, b, c] = [e.v0, e.v1, third].map(|i| mesh.vertices[usize::from(i)]);
            let left = (b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0]);
            Some(if left >= 0.0 { (e.v0, e.v1) } else { (e.v1, e.v0) })
        })
        .collect();
    let mut outgoing: HashMap<u16, Vec<usize>> = HashMap::new();
    for (index, &(from, _)) in directed.iter().enumerate() {
        outgoing.entry(from).or_default().push(index);
    }
    let mut used = vec![false; directed.len()];
    let mut chains = Vec::new();
    for first in 0..directed.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let (start, mut current) = directed[first];
        let mut indices = vec![start, current];
        let mut closed = false;
        while current != start {
            let next = outgoing
                .get(&current)
                .and_then(|edges| edges.iter().copied().find(|&e| !used[e]));
            let Some(next) = next else { break };
            used[next] = true;
            current = directed[next].1;
            if current == start {
                closed = true;
            } else {
                indices.push(current);
            }
        }
        chains.push((indices.iter().map(|&i| mesh.vertices[usize::from(i)]).collect(), closed));
    }
    chains
}

/// Chaikin corner cutting in x/z (heights interpolated alongside). Open chains
/// keep their end points so they still meet the chains they touch.
fn round_corners(mut points: Vec<[f32; 3]>, closed: bool) -> Vec<[f32; 3]> {
    for _ in 0..SMOOTH_PASSES {
        let n = points.len();
        if n < 3 {
            break;
        }
        let segments = if closed { n } else { n - 1 };
        let mut next = Vec::with_capacity(2 * segments + 2);
        if !closed {
            next.push(points[0]);
        }
        for i in 0..segments {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let length = (b[0] - a[0]).hypot(b[2] - a[2]);
            let k = if length > 0.0 { (SMOOTH_CUT_METRES / length).min(0.25) } else { 0.25 };
            let lerp = |t: f32| [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * t);
            if closed || i > 0 {
                next.push(lerp(k));
            }
            if closed || i + 1 < segments {
                next.push(lerp(1.0 - k));
            }
        }
        if !closed {
            next.push(points[n - 1]);
        }
        points = next;
    }
    points
}

/// Pixels whose centre lies inside the outline (non-zero winding, so
/// overlapping storeys union and holes stay open).
fn rasterize_outline(walls: &[[[f32; 3]; 2]], projection: Projection, width: usize, height: usize) -> Vec<bool> {
    let mut crossings: Vec<Vec<(f32, i32)>> = vec![Vec::new(); height];
    for [a, b] in walls {
        let ((ax, ay), (bx, by)) = (projection.pixel(*a), projection.pixel(*b));
        if ay == by {
            continue;
        }
        let (dir, (x0, y0), (x1, y1)) = if ay < by { (1, (ax, ay), (bx, by)) } else { (-1, (bx, by), (ax, ay)) };
        let start = (y0 - 0.5).ceil().clamp(0.0, height as f32) as usize;
        let end = (y1 - 0.5).ceil().clamp(0.0, height as f32) as usize;
        for (row, list) in crossings.iter_mut().enumerate().take(end).skip(start) {
            let cy = row as f32 + 0.5;
            list.push((x0 + (cy - y0) * (x1 - x0) / (y1 - y0), dir));
        }
    }
    let mut inside = vec![false; width * height];
    for (row, list) in crossings.iter_mut().enumerate() {
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for pair in list.windows(2) {
            winding += pair[0].1;
            if winding == 0 {
                continue;
            }
            let from = (pair[0].0 - 0.5).ceil().clamp(0.0, width as f32) as usize;
            let to = (pair[1].0 - 0.5).ceil().clamp(0.0, width as f32) as usize;
            inside[row * width + from..row * width + to].fill(true);
        }
    }
    inside
}

/// Drops floor outside the rounded outline and extends heights into the
/// outline's rounded-out corners from neighbouring floor.
fn clip_to_outline(heights: &mut [f32], inside: &[bool], width: usize, height: usize) {
    for (h, &inside) in heights.iter_mut().zip(inside) {
        if !inside {
            *h = f32::NEG_INFINITY;
        }
    }
    let mut missing: Vec<usize> = (0..heights.len()).filter(|&i| inside[i] && !heights[i].is_finite()).collect();
    while !missing.is_empty() {
        let filled: Vec<(usize, f32)> = missing
            .iter()
            .filter_map(|&i| {
                let (x, y) = (i % width, i / width);
                let neighbours = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < width).then(|| i + 1),
                    (y > 0).then(|| i - width),
                    (y + 1 < height).then(|| i + width),
                ];
                let found: Vec<f32> =
                    neighbours.into_iter().flatten().map(|n| heights[n]).filter(|h| h.is_finite()).collect();
                (!found.is_empty()).then(|| (i, found.iter().sum::<f32>() / found.len() as f32))
            })
            .collect();
        if filled.is_empty() {
            break; // enclosed by the outline but never reached by floor
        }
        filled.iter().for_each(|&(i, h)| heights[i] = h);
        missing.retain(|&i| !heights[i].is_finite());
    }
}

/// Anti-aliased coverage (0..1) of wall segments, skipping walls of storeys
/// hidden under the floor drawn at that pixel.
fn rasterize_walls(
    walls: &[[[f32; 3]; 2]],
    projection: Projection,
    floor: &[f32],
    width: usize,
    height: usize,
) -> Vec<f32> {
    let half = WALL_WIDTH_PX / 2.0;
    let mut cover = vec![0.0f32; width * height];
    for &[a, b] in walls {
        let ((ax, ay), (bx, by)) = (projection.pixel(a), projection.pixel(b));
        let (min_x, max_x) = pixel_range(ax.min(bx) - half - 1.0, ax.max(bx) + half + 1.0, width);
        let (min_y, max_y) = pixel_range(ay.min(by) - half - 1.0, ay.max(by) + half + 1.0, height);
        let (dx, dy) = (bx - ax, by - ay);
        let length_sq = (dx * dx + dy * dy).max(f32::EPSILON);
        for py in min_y..max_y {
            for px in min_x..max_x {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let t = (((cx - ax) * dx + (cy - ay) * dy) / length_sq).clamp(0.0, 1.0);
                if a[1] + t * (b[1] - a[1]) < floor[py * width + px] - HIDDEN_WALL_METRES {
                    continue;
                }
                let distance = ((cx - ax - t * dx).powi(2) + (cy - ay - t * dy).powi(2)).sqrt();
                let slot = &mut cover[py * width + px];
                *slot = slot.max((half + 0.5 - distance).clamp(0.0, 1.0));
            }
        }
    }
    cover
}

fn pixel_range(lo: f32, hi: f32, limit: usize) -> (usize, usize) {
    let start = lo.floor().max(0.0) as usize;
    let end = (hi.ceil() + 1.0).clamp(0.0, limit as f32) as usize;
    (start.min(limit), end)
}

fn ramp(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for pair in HEIGHT_RAMP.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            let k = (t - a) / (b - a);
            return [0, 1, 2].map(|i| ca[i] + (cb[i] - ca[i]) * k);
        }
    }
    HEIGHT_RAMP[HEIGHT_RAMP.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(path: &str, vertices: u32, edges: u32, triangles: u32) -> Vec<u8> {
        let mut bytes = vec![0xAB; 16];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let payload: Vec<u32> = vec![0xDEAD_BEEF, vertices, edges, 9, 0, 0, triangles, 10];
        bytes.extend_from_slice(&((payload.len() * 4) as u32).to_le_bytes());
        payload.iter().for_each(|v| bytes.extend_from_slice(&v.to_le_bytes()));
        bytes
    }

    /// A 10 x 10 m square at height 0..2, split into two triangles.
    fn square_body() -> Vec<u8> {
        let mut body = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 2.0, 10.0], [0.0, 2.0, 10.0]] {
            v.iter().for_each(|f| body.extend_from_slice(&f.to_le_bytes()));
        }
        let ff = NO_TRIANGLE;
        // diagonal 0-2 is shared; the four sides are walls
        for e in [[0u16, 1, 0, ff], [1, 2, 0, ff], [2, 0, 0, 1], [2, 3, 1, ff], [3, 0, 1, ff]] {
            e.iter().chain([&ff]).for_each(|v| body.extend_from_slice(&v.to_le_bytes()));
        }
        for t in [[0u16, 1, 2], [2, 3, 4]] {
            t.iter().for_each(|v| body.extend_from_slice(&v.to_le_bytes()));
        }
        body.extend_from_slice(&[0x55; 32]); // trailing navigation data
        body
    }

    #[test]
    fn header_counts_follow_the_dependency_path() {
        let counts = nav_counts(&header("/SF/Levels/Procs/Crypt/CryptBossCircade/0.nav", 827, 1625, 823)).unwrap();
        assert_eq!(counts, NavCounts { vertices: 827, edges: 1625, triangles: 823 });
        assert!(nav_counts(&header("/x/0.nav", 0, 0, 0)).is_err());
        assert!(nav_counts(&[0; 20]).is_err());
    }

    #[test]
    fn body_parses_vertices_edges_and_triangles() {
        let counts = NavCounts { vertices: 4, edges: 5, triangles: 2 };
        let mesh = parse_nav(counts, &square_body()).unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(mesh.edges.iter().filter(|e| e.is_wall()).count(), 4);
        // Counts larger than the body are rejected rather than read past the end.
        assert!(parse_nav(NavCounts { triangles: 40, ..counts }, &square_body()).is_err());
        // A triangle pointing at an edge it does not own is rejected.
        let mut bad = square_body();
        let table = 4 * 12 + 5 * 10;
        bad[table..table + 2].copy_from_slice(&3u16.to_le_bytes());
        assert!(parse_nav(counts, &bad).is_err());
    }

    #[test]
    fn floor_plan_fills_floor_draws_walls_and_leaves_background() {
        let mesh = parse_nav(NavCounts { vertices: 4, edges: 5, triangles: 2 }, &square_body()).unwrap();
        let plan = render_floor_plan(&mesh);
        let side = ((10.0 + 2.0 * PADDING_METRES) * PIXELS_PER_METRE).ceil() as u32;
        assert_eq!((plan.image.width(), plan.image.height()), (side, side));
        let at = |x: f32, z: f32| {
            let (px, py) = plan.projection.pixel([x, 0.0, z]);
            plan.image.get_pixel(px as u32, py as u32).0
        };
        // Low south edge is slate blue, high north edge is sand.
        let (south, north) = (at(5.0, 0.5), at(5.0, 9.5));
        assert!(south[2] > south[0] && north[0] > north[2], "{south:?} {north:?}");
        assert_eq!(at(5.0, 0.0), WALL.map(|c| c as u8));
        assert_eq!(at(-1.0, -1.0), BACKGROUND.map(|c| c as u8));
        // Corners are rounded off: the square's corner tip is no longer floor.
        assert_eq!(at(0.2, 0.2), BACKGROUND.map(|c| c as u8));
        assert_eq!(plan.projection.pixels_per_metre, PIXELS_PER_METRE);
        let alpha_at = |x: f32, z: f32| {
            let (px, py) = plan.projection.pixel([x, 0.0, z]);
            plan.outline_image.get_pixel(px as u32, py as u32).0[3]
        };
        assert_eq!(alpha_at(5.0, 5.0), 0, "floor stays transparent");
        assert_eq!(alpha_at(-1.0, -1.0), 0, "background stays transparent");
        assert!(alpha_at(5.0, 0.0) > 250, "wall remains opaque");
        assert!(!plan.outline.is_empty());
    }

    #[test]
    fn huge_rooms_are_scaled_to_the_size_cap() {
        let mut mesh = parse_nav(NavCounts { vertices: 4, edges: 5, triangles: 2 }, &square_body()).unwrap();
        mesh.vertices.iter_mut().for_each(|v| (v[0], v[2]) = (v[0] * 100.0, v[2] * 100.0));
        let plan = render_floor_plan(&mesh);
        assert!(plan.image.width() <= MAX_IMAGE_SIDE as u32 + 1);
        assert!(plan.projection.pixels_per_metre < PIXELS_PER_METRE);
    }

    #[test]
    fn discovers_room_navmeshes_only() {
        let rooms = discover_rooms(
            [
                "SF/Levels/Procs/Mines/MinesSpawnGo/0.nav",
                "SF/Levels/Procs/Mines/MinesSpawnGo/0.nav",
                "SF/Levels/Procs/Mines/MinesConLifts/MoverNavVolume00.nav",
                "SF/Levels/Procs/Mines/MinesSpawnGo/LevelCapture_mm.png",
                "SF/Levels/Hubs/FeyHub/0.nav",
            ]
            .into_iter(),
        );
        assert_eq!(rooms.len(), 1);
        assert_eq!((rooms[0].set.as_str(), rooms[0].room.as_str()), ("Mines", "MinesSpawnGo"));
    }

    #[test]
    fn room_kind_reads_token_after_set_prefix() {
        for (room, kind) in [
            ("FFxConBuriedHall", Some("connector")),
            ("GoAxDeadendSpring", Some("dead_end")),
            ("MinesDeadendStorehouse", Some("dead_end")),
            ("UnderCityIntGaol", Some("interior")),
            ("UnderCityExitStreamingTileA", Some("exit")),
            ("NEAxStreamingEntranceA", Some("streaming")),
            ("UnderCityEntranceHall", Some("entrance")),
            ("GoAxCaveCapShrine", Some("cap")),
            ("GoAxSpecialCheckpointA", Some("checkpoint")),
            ("CryptBossCircade", Some("boss")),
            ("UnderCityObjTotem", Some("objective")),
            ("CryptSpawnGallery", Some("spawn")),
            ("NEAxBromiusBattleArena", None),
            ("OSCxTC2026xCorridorA", None),
        ] {
            assert_eq!(room_kind(room), kind, "{room}");
        }
    }
}
