//! `objv-convert` — turn a photogrammetry OBJ into a compact OBJV mesh.
//!
//! Pipeline: read the ASCII OBJ, keep only geometry (positions + triangles,
//! ignoring texcoords/materials), subtract an `f64` origin so coordinates fit
//! `f32`, compute per-vertex normals (the source has none), then pack and
//! zstd-compress. A 1 GB CloudCompare OBJ collapses to tens of MB.
//!
//! Usage:
//!   objv-convert <input.obj> [output.objv] [--level N] [--no-compress]

use std::time::Instant;

use objv_format::{write_header, Codec, EncodeOptions};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

struct Args {
    input: String,
    output: String,
    level: i32,
    compress: bool,
    quantize: bool,
    normals: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut level: i32 = 9;
    let mut compress = true;
    let mut quantize = true;
    let mut normals = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--level" => {
                let v = it.next().ok_or("--level needs a value")?;
                level = v.parse().map_err(|_| format!("bad --level value: {v}"))?;
            }
            "--no-compress" => compress = false,
            // --f32: keep full-precision positions (no u16 quantization).
            "--f32" => quantize = false,
            // --normals: store per-vertex normals instead of deriving in-shader.
            "--normals" => normals = true,
            "-h" | "--help" => {
                println!("usage: objv-convert <input.obj> [output.objv] [--level N] [--no-compress] [--f32] [--normals]");
                std::process::exit(0);
            }
            other if other.starts_with("--") => return Err(format!("unknown flag: {other}")),
            other => {
                if input.is_none() {
                    input = Some(other.to_string());
                } else if output.is_none() {
                    output = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument: {other}"));
                }
            }
        }
    }

    let input = input.ok_or("missing input .obj path")?;
    let output = output.unwrap_or_else(|| default_output(&input));
    Ok(Args {
        input,
        output,
        level,
        compress,
        quantize,
        normals,
    })
}

fn default_output(input: &str) -> String {
    match input.rfind('.') {
        Some(i) => format!("{}.objv", &input[..i]),
        None => format!("{input}.objv"),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let t0 = Instant::now();

    let in_bytes = std::fs::metadata(&args.input).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "reading {} ({:.1} MB) ...",
        args.input,
        in_bytes as f64 / 1e6
    );
    let data = std::fs::read(&args.input)?;
    let text = std::str::from_utf8(&data).map_err(|_| "input is not valid UTF-8/ASCII")?;

    let parse = objv_obj::obj_to_mesh(text);
    let mesh = parse.mesh;
    eprintln!(
        "  parsed {} vertices, {} triangles ({:.1}s){}",
        mesh.vertex_count(),
        mesh.triangle_count(),
        t0.elapsed().as_secs_f64(),
        if parse.skipped_lines > 0 {
            format!(", skipped {} malformed line(s)", parse.skipped_lines)
        } else {
            String::new()
        }
    );
    drop(data); // release the source buffer; `mesh` owns its own copies
    if parse.dropped_triangles > 0 {
        eprintln!(
            "  warning: dropped {} triangle(s) with out-of-range indices",
            parse.dropped_triangles
        );
    }
    eprintln!(
        "  origin (subtracted): [{:.3}, {:.3}, {:.3}]",
        mesh.origin[0], mesh.origin[1], mesh.origin[2]
    );
    eprintln!(
        "  local bbox: x[{:.2}, {:.2}] y[{:.2}, {:.2}] z[{:.2}, {:.2}]  span {:.1} x {:.1} x {:.1} m",
        mesh.bbox_min[0], mesh.bbox_max[0],
        mesh.bbox_min[1], mesh.bbox_max[1],
        mesh.bbox_min[2], mesh.bbox_max[2],
        mesh.bbox_max[0] - mesh.bbox_min[0],
        mesh.bbox_max[1] - mesh.bbox_min[1],
        mesh.bbox_max[2] - mesh.bbox_min[2],
    );

    let opts = EncodeOptions {
        quantize_positions: args.quantize,
        store_normals: args.normals,
    };
    eprintln!(
        "  encoding: positions={}, normals={}",
        if args.quantize { "u16 quantized" } else { "f32" },
        if args.normals { "stored" } else { "derived in shader" }
    );
    let payload = mesh.to_payload(opts);
    let ulen = payload.len() as u64;

    let (body, codec) = if args.compress {
        eprintln!(
            "  compressing {:.1} MB payload (zstd level {}) ...",
            payload.len() as f64 / 1e6,
            args.level
        );
        let c = zstd::stream::encode_all(&payload[..], args.level)?;
        (c, Codec::Zstd)
    } else {
        (payload, Codec::None)
    };

    let mut out = Vec::with_capacity(objv_format::FILE_HEADER_LEN + body.len());
    write_header(&mut out, ulen, codec);
    out.extend_from_slice(&body);
    std::fs::write(&args.output, &out)?;

    let out_len = out.len() as u64;
    eprintln!(
        "wrote {} ({:.1} MB){} in {:.1}s",
        args.output,
        out_len as f64 / 1e6,
        if codec != Codec::None {
            format!(
                ", {:.0}x smaller than source, payload {:.1} MB",
                in_bytes as f64 / out_len as f64,
                ulen as f64 / 1e6
            )
        } else {
            String::new()
        },
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
