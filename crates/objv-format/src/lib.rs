//! On-disk container for OBJV compact meshes.
//!
//! A photogrammetry OBJ stores geometry as ASCII text with ~18 significant
//! digits per coordinate, in world coordinates (UTM, values in the millions)
//! that overflow `f32` precision. OBJV fixes both, and adds the quantization
//! that makes geometry actually compressible:
//!
//!   * a single `f64` **origin** is subtracted from every vertex, so local
//!     coordinates fit `f32` with sub-millimetre accuracy over a multi-km site;
//!   * positions are optionally **quantized to `u16`** per axis (against the
//!     local bounding box) — raw `f32` is high-entropy and barely compresses,
//!     whereas `u16` both halves the size and lets zstd find real redundancy;
//!   * normals are **optional** — the viewer can reconstruct them per-pixel
//!     from screen-space derivatives, so the default web export omits them;
//!   * the whole payload is then (optionally) zstd-compressed by the caller.
//!
//! File framing (little-endian):
//! ```text
//!   magic    b"OBJV"          4
//!   version  u16 = 2          2
//!   flags    u16              2   bit0 = payload body is a zstd frame
//!   ulen     u64              8   uncompressed payload length (bytes)
//!   payload  ...                  raw payload, or a zstd frame of it
//! ```
//!
//! Payload is self-describing (see [`Mesh::to_payload`]):
//! ```text
//!   attr      u8         bit0 = positions are u16-quantized; bit1 = normals present
//!   _pad      u8 * 3     reserved, zero
//!   origin    [f64;3]    world point subtracted from every vertex
//!   bbox_min  [f32;3]    local-space bounds (also the quantization offset)
//!   bbox_max  [f32;3]    local-space bounds (also the quantization extent)
//!   vcount    u32
//!   icount    u32        index count (triangles * 3)
//!   positions  vcount*3 * (u16 if quantized else f32)
//!   normals    vcount*3 * f32      (only when attr bit1 set)
//!   indices    icount  * u32
//! ```

#![forbid(unsafe_code)]

pub const MAGIC: [u8; 4] = *b"OBJV";
pub const VERSION: u16 = 2;
pub const FILE_HEADER_LEN: usize = 16;

/// Payload compression codec, stored in the low 2 bits of the file flags.
/// The native CLI emits `Zstd` (best ratio); the in-browser converter emits
/// `Deflate` (pure-Rust, no C). The viewer decodes whichever it finds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    None,
    Zstd,
    Deflate,
}

impl Codec {
    fn to_flags(self) -> u16 {
        match self {
            Codec::None => 0,
            Codec::Zstd => 1,
            Codec::Deflate => 2,
        }
    }
    fn from_flags(f: u16) -> Codec {
        match f & 0x3 {
            1 => Codec::Zstd,
            2 => Codec::Deflate,
            _ => Codec::None,
        }
    }
}

const ATTR_QUANT_POS: u8 = 0x01;
const ATTR_HAS_NORMALS: u8 = 0x02;

/// How to encode geometry into the payload.
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    /// Quantize positions to `u16` per axis (smaller, web default).
    pub quantize_positions: bool,
    /// Store per-vertex normals (off by default; viewer derives them).
    pub store_normals: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            quantize_positions: true,
            store_normals: false,
        }
    }
}

/// A decoded mesh: local-space geometry plus the `f64` origin needed to map it
/// back to world (UTM) coordinates. After decoding, `positions` is always
/// dequantized `f32`; `normals` is empty when the file stored none.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// World point subtracted from every vertex. Add it back for UTM coords.
    pub origin: [f64; 3],
    /// Local-space axis-aligned bounding box.
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Local vertex positions, flattened `[x,y,z, x,y,z, ...]`.
    pub positions: Vec<f32>,
    /// Unit per-vertex normals (same length as `positions`), or empty.
    pub normals: Vec<f32>,
    /// Triangle list; every three entries index one triangle.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Serialize geometry to the uncompressed payload byte layout.
    pub fn to_payload(&self, opts: EncodeOptions) -> Vec<u8> {
        let vcount = self.vertex_count() as u32;
        let icount = self.indices.len() as u32;
        let store_normals = opts.store_normals && self.normals.len() == self.positions.len();

        let mut attr = 0u8;
        if opts.quantize_positions {
            attr |= ATTR_QUANT_POS;
        }
        if store_normals {
            attr |= ATTR_HAS_NORMALS;
        }

        let pos_bytes = if opts.quantize_positions { 2 } else { 4 };
        let mut out = Vec::with_capacity(
            4 + 24 + 24 + 8
                + self.positions.len() * pos_bytes
                + if store_normals { self.normals.len() * 4 } else { 0 }
                + self.indices.len() * 4,
        );

        out.push(attr);
        out.extend_from_slice(&[0u8; 3]); // pad
        for v in self.origin {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.bbox_min {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.bbox_max {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&vcount.to_le_bytes());
        out.extend_from_slice(&icount.to_le_bytes());

        if opts.quantize_positions {
            let scale = quant_scale(self.bbox_min, self.bbox_max);
            for i in 0..self.positions.len() {
                let axis = i % 3;
                let q = quantize(self.positions[i], self.bbox_min[axis], scale[axis]);
                out.extend_from_slice(&q.to_le_bytes());
            }
        } else {
            for &v in &self.positions {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        if store_normals {
            for &v in &self.normals {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for &v in &self.indices {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Parse a mesh from an uncompressed payload, dequantizing positions back
    /// to `f32` (the inverse of [`to_payload`]).
    pub fn from_payload(buf: &[u8]) -> Result<Mesh, FormatError> {
        let mut r = Reader::new(buf);
        let attr = r.u8()?;
        let _ = r.take::<3>()?; // pad
        let quantized = attr & ATTR_QUANT_POS != 0;
        let has_normals = attr & ATTR_HAS_NORMALS != 0;

        let origin = [r.f64()?, r.f64()?, r.f64()?];
        let bbox_min = [r.f32()?, r.f32()?, r.f32()?];
        let bbox_max = [r.f32()?, r.f32()?, r.f32()?];
        let vcount = r.u32()? as usize;
        let icount = r.u32()? as usize;

        let mut positions = vec![0.0f32; vcount * 3];
        if quantized {
            let scale = quant_scale(bbox_min, bbox_max);
            for (i, p) in positions.iter_mut().enumerate() {
                let axis = i % 3;
                *p = dequantize(r.u16()?, bbox_min[axis], scale[axis]);
            }
        } else {
            for p in positions.iter_mut() {
                *p = r.f32()?;
            }
        }

        let mut normals = Vec::new();
        if has_normals {
            normals = vec![0.0f32; vcount * 3];
            for n in normals.iter_mut() {
                *n = r.f32()?;
            }
        }

        let mut indices = vec![0u32; icount];
        for i in indices.iter_mut() {
            *i = r.u32()?;
        }
        Ok(Mesh {
            origin,
            bbox_min,
            bbox_max,
            positions,
            normals,
            indices,
        })
    }
}

/// Per-axis quantization scale: world units per quantization step.
fn quant_scale(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
    let mut s = [1.0f32; 3];
    for k in 0..3 {
        let span = max[k] - min[k];
        s[k] = if span > 0.0 { span / 65535.0 } else { 1.0 };
    }
    s
}

fn quantize(v: f32, min: f32, scale: f32) -> u16 {
    let q = ((v - min) / scale).round();
    q.clamp(0.0, 65535.0) as u16
}

fn dequantize(q: u16, min: f32, scale: f32) -> f32 {
    min + q as f32 * scale
}

/// Write the 16-byte file header followed by `body`.
///
/// `body` is the payload encoded with `codec`; pass the *uncompressed* payload
/// length in `ulen`.
pub fn write_header(out: &mut Vec<u8>, ulen: u64, codec: Codec) {
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&codec.to_flags().to_le_bytes());
    out.extend_from_slice(&ulen.to_le_bytes());
}

/// Parsed file header plus the offset at which the payload body begins.
#[derive(Clone, Copy, Debug)]
pub struct FileHeader {
    pub version: u16,
    pub codec: Codec,
    /// Uncompressed payload length in bytes.
    pub ulen: usize,
    /// Offset of the payload body within the file buffer.
    pub body_offset: usize,
}

/// Validate the magic/version and read the file header.
pub fn read_header(buf: &[u8]) -> Result<FileHeader, FormatError> {
    if buf.len() < FILE_HEADER_LEN {
        return Err(FormatError::Truncated);
    }
    if buf[0..4] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let version = u16::from_le_bytes([buf[4], buf[5]]);
    if version != VERSION {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let flags = u16::from_le_bytes([buf[6], buf[7]]);
    let ulen = u64::from_le_bytes(buf[8..16].try_into().unwrap()) as usize;
    Ok(FileHeader {
        version,
        codec: Codec::from_flags(flags),
        ulen,
        body_offset: FILE_HEADER_LEN,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u16),
}

impl core::fmt::Display for FormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FormatError::Truncated => write!(f, "buffer ended before payload was complete"),
            FormatError::BadMagic => write!(f, "not an OBJV file (bad magic)"),
            FormatError::UnsupportedVersion(v) => write!(f, "unsupported OBJV version {v}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// Minimal little-endian cursor over a byte slice.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let end = self.pos + N;
        if end > self.buf.len() {
            return Err(FormatError::Truncated);
        }
        let mut a = [0u8; N];
        a.copy_from_slice(&self.buf[self.pos..end]);
        self.pos = end;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.take::<2>()?))
    }
    fn f64(&mut self) -> Result<f64, FormatError> {
        Ok(f64::from_le_bytes(self.take::<8>()?))
    }
    fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_le_bytes(self.take::<4>()?))
    }
    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.take::<4>()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Mesh {
        Mesh {
            origin: [671000.5, 2578000.25, 700.0],
            bbox_min: [-1.0, -2.0, -3.0],
            bbox_max: [4.0, 5.0, 6.0],
            positions: vec![-1.0, -2.0, -3.0, 4.0, 5.0, 6.0, 0.0, 0.0, 0.0],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn f32_roundtrips_exactly() {
        let mesh = sample();
        let payload = mesh.to_payload(EncodeOptions {
            quantize_positions: false,
            store_normals: true,
        });
        let back = Mesh::from_payload(&payload).unwrap();
        assert_eq!(back.origin, mesh.origin);
        assert_eq!(back.positions, mesh.positions);
        assert_eq!(back.normals, mesh.normals);
        assert_eq!(back.indices, mesh.indices);
    }

    #[test]
    fn quantized_roundtrips_within_tolerance_and_drops_normals() {
        let mesh = sample();
        let payload = mesh.to_payload(EncodeOptions {
            quantize_positions: true,
            store_normals: false,
        });
        let back = Mesh::from_payload(&payload).unwrap();
        assert!(back.normals.is_empty(), "normals should be omitted");
        // Quantization error is bounded by one step (span/65535) per axis.
        for axis in 0..3 {
            let step = (mesh.bbox_max[axis] - mesh.bbox_min[axis]) / 65535.0;
            for v in 0..mesh.vertex_count() {
                let got = back.positions[v * 3 + axis];
                let want = mesh.positions[v * 3 + axis];
                assert!((got - want).abs() <= step, "axis {axis} off by > 1 step");
            }
        }
        // Endpoints quantize exactly to 0 and 65535.
        assert_eq!(back.positions[0], mesh.bbox_min[0]);
        assert_eq!(back.positions[3], mesh.bbox_max[0]);
    }

    #[test]
    fn header_roundtrips() {
        for codec in [Codec::None, Codec::Zstd, Codec::Deflate] {
            let mut buf = Vec::new();
            write_header(&mut buf, 12345, codec);
            let h = read_header(&buf).unwrap();
            assert_eq!(h.codec, codec);
            assert_eq!(h.ulen, 12345);
            assert_eq!(h.body_offset, FILE_HEADER_LEN);
        }
    }
}
