//! Pure geometry kernels for the OBJV analysis tools.
//!
//! Everything operates on flat `&[f32]` position arrays + `&[u32]` index arrays
//! (the same buffers the viewer already holds) and small `[f32; 3]` vectors, so
//! it has no dependencies and is unit-tested on native before running in WASM.
//!
//! Conventions: world axes are UTM-local — **X east, Y north, Z up** — so
//! azimuths are measured clockwise from +Y (north) and dip is the angle of a
//! plane below horizontal.

#![forbid(unsafe_code)]

pub type Vec3 = [f32; 3];

#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 1e-20 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}

#[inline]
fn tri(positions: &[f32], i: u32) -> Vec3 {
    let k = i as usize * 3;
    [positions[k], positions[k + 1], positions[k + 2]]
}

// --- ray casting -----------------------------------------------------------

pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3, // need not be normalized; `t` is in units of `dir`
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub point: Vec3,
    pub triangle: u32,
    pub normal: Vec3,
}

/// Closest ray/mesh intersection by brute-force Möller–Trumbore over every
/// triangle. ~10M tests per call — fine for click-frequency picking; a BVH
/// would be needed for per-frame hover.
pub fn raycast(positions: &[f32], indices: &[u32], ray: &Ray) -> Option<Hit> {
    const EPS: f32 = 1e-7;
    let mut best: Option<Hit> = None;
    let tri_count = indices.len() / 3;
    for f in 0..tri_count {
        let i0 = indices[f * 3];
        let i1 = indices[f * 3 + 1];
        let i2 = indices[f * 3 + 2];
        let v0 = tri(positions, i0);
        let v1 = tri(positions, i1);
        let v2 = tri(positions, i2);
        let e1 = sub(v1, v0);
        let e2 = sub(v2, v0);
        let p = cross(ray.dir, e2);
        let det = dot(e1, p);
        if det.abs() < EPS {
            continue;
        }
        let inv = 1.0 / det;
        let tvec = sub(ray.origin, v0);
        let u = dot(tvec, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(tvec, e1);
        let v = dot(ray.dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = dot(e2, q) * inv;
        if t <= EPS {
            continue;
        }
        if best.map_or(true, |b| t < b.t) {
            best = Some(Hit {
                t,
                point: add(ray.origin, scale(ray.dir, t)),
                triangle: f as u32,
                normal: normalize(cross(e1, e2)),
            });
        }
    }
    best
}

// --- measurements ----------------------------------------------------------

/// Total length of a polyline through `pts`.
pub fn polyline_length(pts: &[Vec3]) -> f32 {
    pts.windows(2).map(|w| length(sub(w[1], w[0]))).sum()
}

/// Area of a (possibly non-planar) polygon via Newell's method.
pub fn polygon_area(pts: &[Vec3]) -> f32 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut n = [0.0f32; 3];
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    0.5 * length(n)
}

// --- plane fitting & orientation ------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub point: Vec3,  // a point on the plane (the centroid for a fit)
    pub normal: Vec3, // unit normal, oriented into the upper hemisphere
}

/// Geological orientation of a plane, in degrees.
#[derive(Clone, Copy, Debug)]
pub struct Orientation {
    /// Dip: angle below horizontal, 0–90°.
    pub dip: f32,
    /// Dip direction: azimuth of steepest descent, 0–360° clockwise from north.
    pub dip_direction: f32,
    /// Strike: dip_direction − 90° (right-hand rule), 0–360°.
    pub strike: f32,
}

/// Best-fit plane through ≥3 points (PCA: normal = eigenvector of the smallest
/// eigenvalue of the covariance matrix).
pub fn fit_plane(pts: &[Vec3]) -> Option<Plane> {
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f32;
    let mut c = [0.0f32; 3];
    for p in pts {
        c = add(c, *p);
    }
    c = scale(c, 1.0 / n);

    // Symmetric covariance matrix (xx, yy, zz, xy, xz, yz).
    let (mut xx, mut yy, mut zz, mut xy, mut xz, mut yz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let d = sub(*p, c);
        xx += d[0] * d[0];
        yy += d[1] * d[1];
        zz += d[2] * d[2];
        xy += d[0] * d[1];
        xz += d[0] * d[2];
        yz += d[1] * d[2];
    }
    let m = [[xx, xy, xz], [xy, yy, yz], [xz, yz, zz]];
    let (evals, evecs) = jacobi_eigen_3x3(m);
    // Smallest eigenvalue → plane normal.
    let mut k = 0;
    for i in 1..3 {
        if evals[i] < evals[k] {
            k = i;
        }
    }
    let mut normal = normalize([evecs[0][k], evecs[1][k], evecs[2][k]]);
    if normal[2] < 0.0 {
        normal = scale(normal, -1.0); // orient up
    }
    Some(Plane { point: c, normal })
}

/// Strike/dip from a plane normal (assumed unit, upper-hemisphere).
pub fn orientation(normal: Vec3) -> Orientation {
    let nz = normal[2].clamp(-1.0, 1.0);
    let dip = nz.acos().to_degrees(); // 0 = horizontal plane, 90 = vertical
    // Steepest-descent horizontal direction is (n.x, n.y); azimuth from north.
    let mut dip_dir = (normal[0]).atan2(normal[1]).to_degrees();
    if dip_dir < 0.0 {
        dip_dir += 360.0;
    }
    let strike = (dip_dir - 90.0).rem_euclid(360.0);
    Orientation {
        dip,
        dip_direction: dip_dir,
        strike,
    }
}

/// Jacobi eigenvalue algorithm for a symmetric 3×3 matrix.
/// Returns (eigenvalues, eigenvectors-as-columns).
fn jacobi_eigen_3x3(mut a: [[f32; 3]; 3]) -> ([f32; 3], [[f32; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        // Largest off-diagonal element.
        let (mut p, mut q) = (0, 1);
        let mut max = a[0][1].abs();
        if a[0][2].abs() > max {
            max = a[0][2].abs();
            p = 0;
            q = 2;
        }
        if a[1][2].abs() > max {
            max = a[1][2].abs();
            p = 1;
            q = 2;
        }
        if max < 1e-12 {
            break;
        }
        let app = a[p][p];
        let aqq = a[q][q];
        let apq = a[p][q];
        let theta = 0.5 * (aqq - app) / apq;
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        // Apply rotation J^T A J and accumulate V J.
        for i in 0..3 {
            let aip = a[i][p];
            let aiq = a[i][q];
            a[i][p] = c * aip - s * aiq;
            a[i][q] = s * aip + c * aiq;
        }
        for i in 0..3 {
            let api = a[p][i];
            let aqi = a[q][i];
            a[p][i] = c * api - s * aqi;
            a[q][i] = s * api + c * aqi;
        }
        for i in 0..3 {
            let vip = v[i][p];
            let viq = v[i][q];
            v[i][p] = c * vip - s * viq;
            v[i][q] = s * vip + c * viq;
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

// --- planar cross-section --------------------------------------------------

/// Intersect a mesh with a plane, returning one segment per crossed triangle.
/// Together the segments form the section profile (possibly in several pieces).
pub fn slice_plane(positions: &[f32], indices: &[u32], plane: &Plane) -> Vec<[Vec3; 2]> {
    let mut segs = Vec::new();
    let tri_count = indices.len() / 3;
    for f in 0..tri_count {
        let vs = [
            tri(positions, indices[f * 3]),
            tri(positions, indices[f * 3 + 1]),
            tri(positions, indices[f * 3 + 2]),
        ];
        let d = [
            dot(sub(vs[0], plane.point), plane.normal),
            dot(sub(vs[1], plane.point), plane.normal),
            dot(sub(vs[2], plane.point), plane.normal),
        ];
        // Collect intersection points on edges whose endpoints straddle 0.
        let mut hits: Vec<Vec3> = Vec::with_capacity(2);
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            let (di, dj) = (d[i], d[j]);
            if (di < 0.0 && dj >= 0.0) || (di >= 0.0 && dj < 0.0) {
                let w = di / (di - dj);
                hits.push(add(vs[i], scale(sub(vs[j], vs[i]), w)));
            }
        }
        if hits.len() == 2 {
            segs.push([hits[0], hits[1]]);
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raycast_hits_a_triangle() {
        // Triangle in the z=0 plane; ray straight down from above.
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices = [0u32, 1, 2];
        let ray = Ray {
            origin: [0.25, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        let hit = raycast(&positions, &indices, &ray).expect("should hit");
        assert!((hit.point[2] - 0.0).abs() < 1e-5);
        assert!((hit.t - 5.0).abs() < 1e-4);
        assert!(hit.normal[2].abs() > 0.99); // normal ~ ±z
    }

    #[test]
    fn raycast_misses_outside() {
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices = [0u32, 1, 2];
        let ray = Ray {
            origin: [5.0, 5.0, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert!(raycast(&positions, &indices, &ray).is_none());
    }

    #[test]
    fn length_and_area() {
        let pts = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 4.0, 0.0]];
        assert!((polyline_length(&pts) - 7.0).abs() < 1e-5);
        // Unit square has area 1.
        let sq = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        assert!((polygon_area(&sq) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn plane_fit_and_dip() {
        // A plane dipping 45° toward the east (+X): z = x.
        // Points on it; normal should be ~ (-1,0,1)/√2 (oriented up).
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [2.0, 0.5, 2.0],
        ];
        let plane = fit_plane(&pts).unwrap();
        let o = orientation(plane.normal);
        assert!((o.dip - 45.0).abs() < 1.0, "dip was {}", o.dip);
        // z = x is higher toward +X (east), so it dips DOWN toward the west =>
        // dip direction 270°, and strike is N–S (180°).
        assert!((o.dip_direction - 270.0).abs() < 1.0, "dipdir {}", o.dip_direction);
        assert!((o.strike - 180.0).abs() < 1.0, "strike {}", o.strike);
    }

    #[test]
    fn slice_a_plane_through_two_triangles() {
        // Two triangles forming a quad in z=0, sliced by the x=0.5 plane.
        let positions = [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let indices = [0u32, 1, 2, 0, 2, 3];
        let plane = Plane {
            point: [0.5, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        };
        let segs = slice_plane(&positions, &indices, &plane);
        assert!(!segs.is_empty());
        for s in &segs {
            for p in s {
                assert!((p[0] - 0.5).abs() < 1e-5, "segment not on x=0.5");
            }
        }
    }
}
