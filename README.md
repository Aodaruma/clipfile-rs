# clipfile

[![CI](https://github.com/Aodaruma/clipfile-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Aodaruma/clipfile-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/clipfile.svg)](https://crates.io/crates/clipfile)
[![docs.rs](https://docs.rs/clipfile/badge.svg)](https://docs.rs/clipfile)

An experimental Rust parser and toolkit for CLIP STUDIO PAINT `.clip` files.

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
- optional, read-only SQLite access with runtime schema discovery; and
- high-level project, canvas, layer, and validated layer-tree models; and
- bounded `CanvasPreview` PNG extraction; and
- bounded retrieval of opaque vector-layer external data; and
- bounded UTF-8 text-layer content with opaque per-object attributes; and
- optional timeline and image-cel selection-curve decoding; and
- optional `Offscreen.Attribute`, zlib tile, and RGBA/grayscale raster decoding.

Not implemented yet:

- semantic vector/text-attribute decoding, non-cel animation curves,
  time-lapse, or `.cmc` support; and
- writing or modifying files.

See [the format analysis](docs/format-analysis.md) and
[the implementation plan](docs/implementation-plan.md) for the research status
and planned API layers. [The model guide](docs/model.md) explains the
high-level SQLite view. Unresolved details are tracked in
[the open-questions log](docs/open-questions.md).

## Installation

The container parser has no default dependencies:

```toml
[dependencies]
clipfile = "0.1"
```

Enable `sqlite` for document metadata, `raster` for SQLite and raster decoding,
or `animation` for timeline and image-cel selection curves:

```toml
[dependencies]
clipfile = { version = "0.1", features = ["raster"] }
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
```

The optional `sqlite` feature uses a bundled SQLite build for reproducible
linking across supported platforms. It provides `ClipFile::open_database`,
runtime table/column discovery, integrity checking, and cross-validation of
the `ExternalChunk` index.

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

`Database::text_layer` validates stored text as UTF-8 and pairs each string
with its original opaque attribute record. Length-prefixed extra-object arrays,
total bytes, and object counts are bounded; font, paragraph, and transform
attributes are not interpreted yet.

The `animation` feature reads validated timeline ranges and resolves
`TrackKind=2000` action mixers to their layer UUIDs. It bounds zlib expansion,
checks the BINC string table and arrays, and exposes sorted `ImageCelName`
keyframes through `Animation`, `Timeline`, and `CelTrack`. Other track kinds
and time-lapse data remain opaque.

The `raster` feature builds on `sqlite`. It resolves a layer render, layer
mask, or mipmap to its base offscreen data, supports bounded tile-by-tile
decompression, and can assemble currently understood 8-bit `(alpha, BGRA)`
or grayscale layouts.
Unknown and bit-packed layouts return an explicit unsupported-format error.

The API is intentionally read-only at this stage. Treat all input as
untrusted; the parser validates structural bounds, but coverage of the full
format is still incomplete.

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
