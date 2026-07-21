# clipfile

An experimental Rust parser and toolkit for CLIP STUDIO PAINT `.clip` files.

The file format is proprietary and does not have a public official
specification. The crate therefore begins with a small, conservative API for
the structure that can be validated reliably.

## Current status

Implemented:

- streaming parsing of the `CSFCHUNK` envelope;
- top-level `CHNKHead`, `CHNKExta`, `CHNKSQLi`, and `CHNKFoot` discovery;
- strict size, offset, and chunk-order validation;
- bounded or streaming access to chunk payloads; and
- parsing of file and external-object headers;
- classification of block data, length-prefixed zlib streams, and audio; and
- block indexing without loading compressed tile payloads; and
- optional, read-only SQLite access with runtime schema discovery.

Not implemented yet:

- high-level SQLite-backed document and layer models;
- raster tile decoding as a public API;
- vector, text, animation, or `.cmc` support; and
- writing or modifying files.

See [the format analysis](docs/format-analysis.md) and
[the implementation plan](docs/implementation-plan.md) for the research status
and planned API layers. Unresolved details are tracked in
[the open-questions log](docs/open-questions.md).

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
```

The optional `sqlite` feature uses a bundled SQLite build for reproducible
linking across supported platforms. It provides `ClipFile::open_database`,
runtime table/column discovery, integrity checking, and cross-validation of
the `ExternalChunk` index.

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
