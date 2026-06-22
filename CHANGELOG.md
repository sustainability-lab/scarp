# Changelog

All notable changes to Scarp. Newest first.

## 2026-06-22

### Added
- **Geographic OBJ support** — lon/lat (degree) photogrammetry exports (e.g.
  Agisoft Metashape) are detected and projected to a local metre frame, so they
  render with correct proportions instead of a thin vertical sliver.
- **Lossless compression** — first-use vertex reorder + delta/zigzag/varint
  coding of indices and positions. Wadi Birk sample: **84 MB → 60 MB** (bit-exact).
- **In-browser conversion** — drop a `.obj` and it converts in a Web Worker
  (responsive UI + progress bar) to a downloadable `.objv`. Drop `.objv` to view.
- **Analysis tools** — measure distance/area, vertical cross-section profiles,
  and strike/dip plane-fit.
- **Colormaps + legend** — shaded relief, elevation, slope, aspect.
- **This changelog** and a "last deployed" stamp on the page.

### Fixed
- Loading a second file no longer reloads the page / reverts to the sample mesh.
- The download link wraps inside its panel instead of overflowing.
- Cache-busting so deploys are picked up without a manual hard-refresh.

### Notes
- Renamed the project to **Scarp**; the compact mesh format keeps the `.objv`
  extension.
- Hosted on GitHub Pages, fully client-side — your data never leaves the browser.
