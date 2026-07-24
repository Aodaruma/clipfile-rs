# clipfile

[![CI](https://github.com/Aodaruma/clipfile-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Aodaruma/clipfile-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/clipfile.svg)](https://crates.io/crates/clipfile)
[![docs.rs](https://docs.rs/clipfile/badge.svg)](https://docs.rs/clipfile)

An experimental Rust parser and toolkit for CLIP STUDIO PAINT `.clip` and
`.cmc` files.

The file format is proprietary and does not have a public official
specification. The crate therefore begins with a small, conservative API for
the structure that can be validated reliably.

## Current status

Implemented:

- streaming parsing of the `CSFCHUNK` envelope;
- top-level `CHNKHead`, `CHNKExta`, `CHNKSQLi`, and `CHNKFoot` discovery;
- strict size, offset, and chunk-order validation;
- bounded or streaming access to chunk payloads;
- parsing of file and external-object headers;
- classification of block data, length-prefixed zlib streams, and audio;
- block indexing without loading compressed tile payloads;
- common opaque block-status reporting for observed uniform containers;
- optional, read-only SQLite access with runtime schema discovery; and
- validated standalone `.cmc` page-management trees and safe page links; and
- high-level project, canvas, layer, and validated layer-tree models; and
- bounded `CanvasPreview` PNG extraction; and
- bounded retrieval of opaque vector-layer external data; and
- validated vector-ruler references and typed metadata for all nine observed
  special-ruler table kinds; and
- bounded UTF-8 text-layer content with opaque per-object attributes; and
- bounded correction-layer decoding for all nine observed adjustment kinds,
  with raw payload preservation; and
- optional timeline, generic primary and double-precision secondary
  action-mixer curves, image-cel selection, audio/play-time curve decoding,
  and typed inline track values; and
- validated 2D-camera layers, current transform snapshots, typed track values,
  and axis-qualified position/center curves; and
- optional validated time-lapse manager/record/blob chains with bounded reads
  and streaming decompression, plus streaming internal WebP frame indexing; and
- optional `Offscreen.Attribute`, zlib tile, and RGBA/grayscale raster decoding;
  and
- opt-in validated container rewriting with editable SQLite metadata,
  byte-preserving external bodies, repaired absolute offsets, CSP-compatible
  block checksums, and conservative image/vector/text/animation edits.

Not implemented yet:

- semantic vector/text-attribute decoding, 3D data, or time-lapse
  playback/timestamp semantics; and
- arbitrary object creation and full-fidelity style/stroke/track encoders for
  raster, vector, text, animation, and other external object bodies.

See [the format analysis](docs/format-analysis.md) and
[the implementation plan](docs/implementation-plan.md) for the research status
and planned API layers. [The model guide](docs/model.md) explains the
high-level SQLite view. Unresolved details are tracked in
[the open-questions log](docs/open-questions.md).

## Installation

The container parser has no default dependencies:

```toml
[dependencies]
clipfile = "0.5"
```

Enable `sqlite` for document metadata, `raster` for SQLite and raster decoding,
`animation` for timeline curves, `timelapse` for time-lapse metadata and
payload access, or `write` for validated low-level rewriting:

```toml
[dependencies]
clipfile = { version = "0.5", features = ["raster", "animation", "timelapse"] }
```

## Example

```rust,no_run
use std::fs::File;

use clipfile::ClipFile;

let mut clip = ClipFile::open(File::open("drawing.clip")?)?;
let summary = clip.validate()?;

println!("external chunks: {}", summary.external_chunks());
println!("SQLite bytes: {}", summary.database_payload_size());

# Ok::<(), Box<dyn std::error::Error>>(())
```

To inspect a local file without loading its large payloads into memory:

```console
cargo run --example inspect -- path/to/drawing.clip
cargo run --example inspect -- path/to/drawing.clip --deep
cargo run --features sqlite --example inspect -- path/to/drawing.clip --database
cargo run --features sqlite --example inspect -- path/to/drawing.clip --document
cargo run --features raster --example inspect -- path/to/drawing.clip --raster
cargo run --features animation --example inspect -- path/to/drawing.clip --animation
cargo run --features timelapse --example inspect -- path/to/drawing.clip --timelapse
cargo run --features sqlite --example inspect_cmc -- path/to/project.cmc
cargo run --features sqlite --example inspect_corrections -- path/to/drawing.clip
cargo run --features sqlite --example inspect_rulers -- path/to/drawing.clip
cargo run --features write --example rewrite -- input.clip new-output.clip
cargo run --features "write,raster" --example invert_first_tile -- input.clip new-output.clip 42
```

Purpose-specific examples, required features, and safety notes are listed in
[the examples guide](examples/README.md).

The optional `sqlite` feature uses a bundled SQLite build for reproducible
linking across supported platforms. It provides `ClipFile::open_database`,
runtime table/column discovery, integrity checking, and cross-validation of
the `ExternalChunk` index.

The same feature provides `CmcFile::open` for standalone page-management
files. It validates the `Project` row, the complete `CanvasNode` child/sibling
tree, positive and existing references, cycles, multiple parents, reachability,
and a configurable node limit. Observed `.:page.clip` links can be resolved
relative to the `.cmc` directory; unknown or traversal-capable link forms stay
available as raw text but are not converted into filesystem paths.

With the same feature, `ClipFile::read_document` builds a `Document` with
project/canvas metadata, core layer properties, mipmap references, and a
validated `LayerTree`. Raw flags and numeric kinds remain available so newer
format values are not discarded. `Database::canvas_preview` returns the
encoded preview for a canvas after applying a size limit and cross-checking
PNG IHDR dimensions when the stored bytes are PNG.

`Database::vector_data_sources` resolves the `VectorObjectList` rows owned by
a layer. `ClipFile::read_vector_data` then retrieves each opaque external body
under configurable row-count and byte-size limits. The bytes are intentionally
not interpreted until the vector-body structure is independently verified.

`Database::ruler_layer` validates a layer's vector-ruler reference or its
`SpecialRulerManager` and linked ruler rows. Parallel, curve, radial,
concentric-circle, guide, perspective, and symmetry metadata are typed; curve
point payloads and perspective vanishing-point guide records are bounded.
Vector-ruler geometry remains available through the opaque vector-data API.

`Database::text_layer` validates stored text as UTF-8 and pairs each string
with its original opaque attribute record. Length-prefixed extra-object arrays,
total bytes, and object counts are bounded; font, paragraph, and transform
attributes are not interpreted yet.

`Database::correction_layer` validates the big-endian `FilterLayerInfo`
section and decodes the nine observed correction kinds: brightness/contrast,
levels, tone curves, hue/saturation/luminosity, color balance, reverse
gradient, posterization, threshold, and gradient map. Raw fixed-point words
and the complete source payload remain available, and future kind values use
an opaque bounded fallback.

The `animation` feature reads validated timeline ranges and resolves tracks to
their layer UUIDs. It validates the complete `FirstTrack` / `TrackNextIndex`
chain, bounds zlib expansion, checks the BINC string table and arrays, and
exposes every `FCurve` in the primary action mixer, including
interpolation, slopes, optional tags, and constant-revision flags. The existing
`CelTrack` view selects the first `ImageCelName` curve for convenient frame
lookup, while `AnimationTrack` preserves raw track kinds and all curves,
including observed `PlayTime` and `AudioPlayer` data. It also validates the
inline `TrackValueMap` and exposes observed floating-point and indexed-text
values while preserving future value types as opaque payloads. Secondary
`0110binc` value records are sparse, use independently validated field
metadata, and preserve their `Double[]` frame, value, and slope arrays as
`f64`. Verified raw-kind helpers cover non-cel folders, image-cel folders,
static-image layers, paper, play-time control, and audio control.

Verified 2D-camera tracks use raw kind `2005`. `ImageCenter` and
`ImagePosition` current values are exposed as two-dimensional values, while
their `Axis=X/Y` primary and secondary curves remain distinct through
`AnimationCurve::axis`. Rotation is expressed in degrees, scale and opacity
as percentages, and the current values reflect the saved timeline position.
`Database::camera_2d_layer` validates the camera folder bit, bounded transform
snapshot, dimensions, finite position/scale/rotation values, and transformed
frame corners;
`ClipFile::read_animation` cross-validates kind-`2005` tracks with those
layers. Unnamed header words and the complete source payload remain available
for forward-compatible inspection.

The `timelapse` feature validates `TimeLapseManager`, `TimeLapseRecord`, and
`TimeLapseBlob` linked lists, including canvas ownership, contiguous decoded
offsets, declared sizes, external-object references, and the observed
big-endian compressed-length prefix. `ClipFile::read_time_lapse_blob` returns
one bounded decoded segment, while `copy_time_lapse_blob` streams it to a
writer. `read_time_lapse_frame_index` streams across all blobs without
retaining image payloads and validates the internal 28-byte records,
one-based sequence, RIFF/WebP boundaries, and observed VP8 dimensions.
`GMIK` records are exposed as full-canvas key frames, while `GMID` records
provide their validated delta-patch destination origin. The raw parameters
remain available because their `GMIK` meaning is not independently verified.
The embedded tables and frame headers contain a contiguous sequence but no
wall-clock timestamp, so the API does not invent real-time playback metadata.

The `raster` feature builds on `sqlite`. It resolves a layer render, layer
mask, or mipmap to its base offscreen data, supports bounded tile-by-tile
decompression, and can assemble currently understood 8-bit `(alpha, BGRA)`
or grayscale layouts.
Unknown and bit-packed layouts return an explicit unsupported-format error.

The opt-in `write` feature provides `ClipFile::writer`. It clones the embedded
SQLite database into writable memory, preserves unchanged external bodies,
repairs `ExternalChunk.Offset` and the `CHNKHead` database offset, and can
replace a complete opaque external body. Existing block data can be
zlib-reencoded with CSP-compatible Adler-32 checksums. With the corresponding
features enabled, higher-level methods replace an existing raster or layer
mask, edit text while preserving encoded character boundaries, translate a
validated vector layout, update existing animation values, keys, and cel tags,
and clone a complete existing Track into an untracked compatible layer with
independent identities and mixer bodies. `write_to_path` creates a new path,
flushes it, then reopens and validates the container, SQLite database, and
external index. It never overwrites an existing path.

This remains a conservative existing-structure editor, not an arbitrary CLIP
document generator. Unknown top-level chunk layouts are rejected, and complete
style, vector-stroke, and template-free animation-track creation are not
implemented. See
[the writing guide](docs/writing.md) for the precise API boundaries and current
guarantees.

Treat all input as untrusted; the parser and writer validate structural bounds,
but coverage of the full format is still incomplete.

## Development

The minimum supported Rust version is 1.85. CI checks stable Rust on Linux,
Windows, and macOS, as well as the declared MSRV. Contributor and release
instructions are in [CONTRIBUTING.md](CONTRIBUTING.md) and
[docs/development.md](docs/development.md).

## Disclaimer

This project is an independent, unofficial implementation and is not
affiliated with, endorsed by, or sponsored by CELSYS, Inc.

CLIP STUDIO PAINT and related names are trademarks or registered
trademarks of CELSYS, Inc.

## License

MIT. See [LICENSE](LICENSE).
