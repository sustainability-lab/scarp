# Scarp — virtual outcrop viewer for the web

[**Live demo →** sustainability-lab.github.io/scarp](https://sustainability-lab.github.io/scarp/)

A fast, open-source, **zero-server** viewer for large photogrammetry meshes
(virtual outcrops — like VRGS / LIME / CloudCompare, but in the browser).
Built in **Rust**, rendered with **wgpu**, shipped as **WASM**. Your data never
leaves the machine: everything (decompress, convert, render, measure) runs
client-side. Meshes are stored in a compact `.objv` container.

> Drop a 1 GB photogrammetry OBJ in your browser → it compresses to tens of MB,
> renders 10M triangles, and you measure distances, cut cross-sections, and read
> strike/dip — with nothing installed and no server.

## Features

- **In-browser conversion** — drop a `.obj` and it's parsed, origin-shifted,
  quantized and compressed to a compact `.objv` you can download. Drop a `.objv`
  and it just views. No CLI, no upload.
- **Renderer** — 2.5 MB wasm. WebGPU with a **WebGL2 fallback**. Handles the full
  10M-triangle Wadi Birk escarpment from an 84 MB file.
- **Colormaps** — shaded relief · elevation · slope · aspect, each with a legend
  (keys `1`–`4`).
- **Analysis tools** (keys `n`/`m`/`s`/`d`):
  - **Measure** — 3D path length, straight distance, Δhoriz/Δz, polygon area.
  - **Cross-section** — slice a vertical plane through the mesh, get the profile
    length and relief.
  - **Strike/dip** — best-fit plane through picked points → geological orientation.

Every layer is verified: unit tests on the geometry kernels and headless-browser
render/interaction checks (`tools/`).

## Why it's small & accurate

A photogrammetry OBJ stores coordinates as ~18-digit ASCII text in world UTM
(values in the millions, which overflow `f32`). OBJV:

1. subtracts an `f64` **origin** so local coords fit `f32` to sub-mm over km;
2. **quantizes** positions to `u16` (raw `f32` barely compresses; integers do);
3. derives normals in-shader (no normal buffer stored);
4. compresses — **zstd** from the CLI, pure-Rust **deflate** in the browser.

Result on the sample dataset: **1.0 GB → 84 MB** (12×), loads and renders smoothly.

## Build & run locally

```bash
# Native converter (optional — the browser can also convert):
cargo build --release -p objv-convert
./target/release/objv-convert your.obj web/your.objv

# Viewer (wasm):
wasm-pack build crates/objv-viewer --target web --release --out-dir web/pkg

# Serve (no backend):
cd web && python3 -m http.server 8848
# open http://localhost:8848/        (loads the bundled sample)
# open http://localhost:8848/?mesh=your.objv
```

Controls: **drag** rotate · **scroll** zoom · **1–4** colormap · **n/m/s/d**
tool · **u** undo point · **x** clear · drop a `.obj`/`.objv` anytime.

## Architecture

```
crates/objv-format   shared .objv container (no deps; native + wasm)
crates/objv-geom     ray-cast, plane-fit, slicing kernels (unit-tested)
crates/objv-obj      OBJ → mesh parser (shared by CLI and browser)
crates/objv-convert  native CLI (zstd)
crates/objv-viewer   wgpu renderer + in-browser converter → WASM
web/                 single-page app (index.html + built pkg/)
tools/               headless verification scripts (playwright-core)
```

Pure-Rust → WASM, no server — the layout follows the GeoLibre
(`opengeos/geolibre-rust`) convention; this is the 3D-rendering counterpart to
its 2D raster/vector processing.

## Hosting

Static — deploys to **GitHub Pages** via `.github/workflows/deploy.yml`
(CI builds the wasm and publishes `web/`). Visitors drop their own files; the
raw/full meshes are never published (a tiny `sample.objv` ships as the demo).

## Roadmap

- [ ] Crush the index buffer further (mesh decimation / LOD + index coding) →
      target < 40 MB and smoother on weak GPUs.
- [ ] Run the in-browser conversion in a Web Worker (keep the UI responsive on
      very large files).
- [ ] Optional glTF / 3D Tiles export for interop with deck.gl / GeoLibre.

## License

MIT
