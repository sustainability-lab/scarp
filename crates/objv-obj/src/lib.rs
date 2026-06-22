//! Parse ASCII OBJ geometry into an OBJV [`Mesh`].
//!
//! Shared by the native CLI and the in-browser (WASM) converter, so it is pure
//! Rust with no I/O: hand it the file text, get back a mesh with a subtracted
//! `f64` origin and computed per-vertex normals. Texcoords, normals and
//! materials in the source are ignored — we want one welded geometry mesh.

#![forbid(unsafe_code)]

use objv_format::Mesh;

/// Result of parsing an OBJ: the mesh plus what was skipped.
pub struct ObjParse {
    pub mesh: Mesh,
    /// Lines that failed to parse as a vertex/face.
    pub skipped_lines: usize,
    /// Triangles dropped for referencing out-of-range vertices.
    pub dropped_triangles: usize,
    /// True if lon/lat degrees were detected and projected to local metres.
    pub projected_geographic: bool,
}

/// Parse OBJ text into a mesh (origin-offset, normals computed,
/// vertices reordered for compression).
pub fn obj_to_mesh(text: &str) -> ObjParse {
    let mut parsed = parse_obj(text);
    let skipped_lines = parsed.skipped;
    // Photogrammetry exports (e.g. Metashape) are sometimes in geographic
    // degrees for X/Y and metres for Z, which renders as a degenerate sliver.
    // Project such data to a local metre frame so proportions are correct.
    let projected_geographic = maybe_project_geographic(&mut parsed.positions_world);
    let (mut mesh, dropped_triangles) = build_mesh(parsed);
    optimize_vertex_fetch(&mut mesh);
    ObjParse {
        mesh,
        skipped_lines,
        dropped_triangles,
        projected_geographic,
    }
}

/// If the X/Y coordinates look like lon/lat degrees (in range, sub-degree span,
/// and a Z range far larger than the horizontal span — the tell-tale of
/// metres-over-degrees), project them in place to a local equirectangular metre
/// frame centred on the data. Returns whether a projection was applied.
fn maybe_project_geographic(pts: &mut [[f64; 3]]) -> bool {
    if pts.is_empty() {
        return false;
    }
    let (mut minx, mut maxx) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut minz, mut maxz) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in pts.iter() {
        minx = minx.min(p[0]);
        maxx = maxx.max(p[0]);
        miny = miny.min(p[1]);
        maxy = maxy.max(p[1]);
        minz = minz.min(p[2]);
        maxz = maxz.max(p[2]);
    }
    let (xspan, yspan, zspan) = (maxx - minx, maxy - miny, maxz - minz);
    let hspan = xspan.max(yspan);
    let in_lonlat = minx >= -180.0 && maxx <= 180.0 && miny >= -90.0 && maxy <= 90.0;
    // Degrees-over-metres tell: tiny horizontal span but a much larger Z range.
    let looks_geographic = in_lonlat && hspan > 0.0 && hspan < 5.0 && zspan > 100.0 * hspan;
    if !looks_geographic {
        return false;
    }
    let lon0 = 0.5 * (minx + maxx);
    let lat0 = 0.5 * (miny + maxy);
    const M_PER_DEG_LAT: f64 = 111_320.0;
    let m_per_deg_lon = M_PER_DEG_LAT * (lat0 * core::f64::consts::PI / 180.0).cos();
    for p in pts.iter_mut() {
        p[0] = (p[0] - lon0) * m_per_deg_lon;
        p[1] = (p[1] - lat0) * M_PER_DEG_LAT;
        // p[2] (Z) already in metres.
    }
    true
}

/// Relabel vertices in the order triangles first reference them (a lossless
/// permutation; positions/normals move with their vertex). This makes adjacent
/// indices numerically close, so the delta-varint index codec — and zstd —
/// compress the index stream far better. Unreferenced vertices are dropped.
fn optimize_vertex_fetch(mesh: &mut Mesh) {
    let nv = mesh.positions.len() / 3;
    if nv == 0 {
        return;
    }
    let has_normals = mesh.normals.len() == mesh.positions.len();
    let mut remap = vec![u32::MAX; nv];
    let mut new_pos = Vec::with_capacity(mesh.positions.len());
    let mut new_nrm = if has_normals {
        Vec::with_capacity(mesh.normals.len())
    } else {
        Vec::new()
    };
    let mut next: u32 = 0;
    for idx in mesh.indices.iter_mut() {
        let old = *idx as usize;
        let mut n = remap[old];
        if n == u32::MAX {
            n = next;
            next += 1;
            remap[old] = n;
            new_pos.extend_from_slice(&mesh.positions[old * 3..old * 3 + 3]);
            if has_normals {
                new_nrm.extend_from_slice(&mesh.normals[old * 3..old * 3 + 3]);
            }
        }
        *idx = n;
    }
    mesh.positions = new_pos;
    if has_normals {
        mesh.normals = new_nrm;
    }
}

struct Parsed {
    positions_world: Vec<[f64; 3]>,
    tris: Vec<[u32; 3]>,
    skipped: usize,
}

/// Parse only `v` and `f` records.
fn parse_obj(text: &str) -> Parsed {
    let mut positions_world: Vec<[f64; 3]> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut skipped = 0usize;
    let mut face_verts: Vec<u32> = Vec::with_capacity(8);

    for line in text.lines() {
        let bytes = line.as_bytes();
        if bytes.len() < 2 {
            continue;
        }
        match (bytes[0], bytes[1]) {
            (b'v', b' ') => {
                let mut it = line[2..].split_whitespace();
                match (
                    it.next().and_then(|s| s.parse::<f64>().ok()),
                    it.next().and_then(|s| s.parse::<f64>().ok()),
                    it.next().and_then(|s| s.parse::<f64>().ok()),
                ) {
                    (Some(x), Some(y), Some(z)) => positions_world.push([x, y, z]),
                    _ => skipped += 1,
                }
            }
            (b'f', b' ') => {
                face_verts.clear();
                let nv = positions_world.len() as i64;
                let mut ok = true;
                for tok in line[2..].split_whitespace() {
                    let first = tok.split('/').next().unwrap_or("");
                    match first.parse::<i64>() {
                        Ok(idx) if idx > 0 => face_verts.push((idx - 1) as u32),
                        Ok(idx) if idx < 0 => face_verts.push((nv + idx) as u32),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok || face_verts.len() < 3 {
                    skipped += 1;
                    continue;
                }
                for k in 1..face_verts.len() - 1 {
                    tris.push([face_verts[0], face_verts[k], face_verts[k + 1]]);
                }
            }
            _ => {}
        }
    }

    Parsed {
        positions_world,
        tris,
        skipped,
    }
}

/// Subtract an origin, convert to `f32` local space, and compute normals.
/// Returns the mesh and the number of triangles dropped for bad indices.
fn build_mesh(parsed: Parsed) -> (Mesh, usize) {
    let Parsed {
        positions_world,
        tris,
        ..
    } = parsed;
    let nv = positions_world.len();

    let mut wmin = [f64::INFINITY; 3];
    let mut wmax = [f64::NEG_INFINITY; 3];
    for p in &positions_world {
        for k in 0..3 {
            wmin[k] = wmin[k].min(p[k]);
            wmax[k] = wmax[k].max(p[k]);
        }
    }
    let origin = [
        ((wmin[0] + wmax[0]) * 0.5).round(),
        ((wmin[1] + wmax[1]) * 0.5).round(),
        ((wmin[2] + wmax[2]) * 0.5).round(),
    ];

    let mut positions = vec![0.0f32; nv * 3];
    let mut bbox_min = [f32::INFINITY; 3];
    let mut bbox_max = [f32::NEG_INFINITY; 3];
    for (i, p) in positions_world.iter().enumerate() {
        for k in 0..3 {
            let v = (p[k] - origin[k]) as f32;
            positions[i * 3 + k] = v;
            bbox_min[k] = bbox_min[k].min(v);
            bbox_max[k] = bbox_max[k].max(v);
        }
    }

    let mut normals = vec![0.0f32; nv * 3];
    let mut bad_index = 0usize;
    for t in &tris {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        if a >= nv || b >= nv || c >= nv {
            bad_index += 1;
            continue;
        }
        let pa = [positions[a * 3], positions[a * 3 + 1], positions[a * 3 + 2]];
        let pb = [positions[b * 3], positions[b * 3 + 1], positions[b * 3 + 2]];
        let pc = [positions[c * 3], positions[c * 3 + 1], positions[c * 3 + 2]];
        let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        for &i in &[a, b, c] {
            normals[i * 3] += n[0];
            normals[i * 3 + 1] += n[1];
            normals[i * 3 + 2] += n[2];
        }
    }
    for i in 0..nv {
        let n = [normals[i * 3], normals[i * 3 + 1], normals[i * 3 + 2]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-12 {
            normals[i * 3] = n[0] / len;
            normals[i * 3 + 1] = n[1] / len;
            normals[i * 3 + 2] = n[2] / len;
        } else {
            normals[i * 3 + 2] = 1.0;
        }
    }

    let mut indices = Vec::with_capacity(tris.len() * 3);
    for t in &tris {
        if (t[0] as usize) < nv && (t[1] as usize) < nv && (t[2] as usize) < nv {
            indices.extend_from_slice(&[t[0], t[1], t[2]]);
        }
    }

    (
        Mesh {
            origin,
            bbox_min,
            bbox_max,
            positions,
            normals,
            indices,
        },
        bad_index,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_quad() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3\nf 1 3 4\n";
        let r = obj_to_mesh(obj);
        assert_eq!(r.mesh.vertex_count(), 4);
        assert_eq!(r.mesh.triangle_count(), 2);
        assert_eq!(r.skipped_lines, 0);
        // Flat quad in z=0 -> all normals point along ±z.
        for i in 0..4 {
            assert!(r.mesh.normals[i * 3 + 2].abs() > 0.99);
        }
    }

    #[test]
    fn ignores_texcoords_and_triangulates_ngons() {
        let obj = "v 0 0 0\nvt 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1 2/1 3/1 4/1\n";
        let r = obj_to_mesh(obj);
        assert_eq!(r.mesh.vertex_count(), 4);
        assert_eq!(r.mesh.triangle_count(), 2); // quad fan -> 2 tris
    }
}
