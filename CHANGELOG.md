# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0-rc.1] - 2026-07-24

### Added

- Template-based plain raster-layer cloning with regenerated row identities,
  layer UUID, external IDs, tree links, CSP-compatible base pixels, and fresh
  non-stale derived-cache references.
- Text-object removal with synchronized primary/additional arrays and safe
  primary-object promotion while retaining at least one object.
- Conservative vector-stroke template cloning and removal for the validated
  92-byte-header/88-byte-point layout, preserving all unknown fields.
- Primary/secondary animation-curve key insertion and removal, normalized
  image-cel Track cloning from a complete kind-2000 template, and Track
  unlinking with timeline-chain repair.
- Purpose-specific examples and safety documentation for each new semantic
  write operation.

### Changed

- Document render-cache boundaries explicitly: raster clones invalidate
  derived references, while vector edits and canvas previews are not
  regenerated automatically.

## [0.5.0] - 2026-07-24

### Added

- Typed constants and stable names for all 29 observed layer blend modes,
  while retaining lossless raw-value construction for future modes.
- Conservative write APIs for per-character-width-preserving text content,
  atomic text-object template cloning with fresh document-wide IDs, validated
  vector translation and opaque bodies, typed animation values, existing
  curve keys, and cel tags.
- CSP-compatible `BlockCheckSum` generation using Adler-32 over the
  little-endian compressed-size prefix and zlib payload, while retaining the
  explicit zero-checksum compatibility mode.
- Safe staging of new external objects before SQLite, including generated
  index rows, repaired offsets, duplicate and limit checks, rollback helpers,
  and strict reopened-output validation.
- Conservative animation Track cloning with regenerated row/track/external
  identities, independent primary and secondary mixer bodies, complete
  unknown-column preservation, and validated timeline-chain insertion.

### Fixed

- Preserve the observed SQLite storage class for newly indexed external
  identifiers so generated files remain readable by CLIP STUDIO PAINT.
- Keep the Track allocation high-water mark in `ElemScheme` synchronized when
  cloning animation tracks.

## [0.4.0] - 2026-07-24

### Added

- Opt-in `write` support with an editable in-memory SQLite database, exact
  no-change round trips, external-body replacement, offset repair, and
  create-new output validation.
- Validated re-encoding of one existing block-data tile with an explicit
  zero-checksum compatibility opt-in and a small raster-editing example.
- Purpose-specific document, preview, text, vector, raster, animation, and
  time-lapse examples with an examples guide and annotated processing steps.

### Fixed

- Keep animation and page-management validation compatible with the declared
  Rust 1.85 minimum version.

### Documentation

- Define full-image, vector, text, and animation semantic writing as the
  priority before a stable 1.0 release, with time-lapse and 3D following later.

## [0.3.0] - 2026-07-24

### Added

- Bounded decoding of inline animation `TrackValueMap` entries, including
  observed floating-point and indexed-text values with opaque fallback.
- Secondary action-mixer presence reporting on `AnimationTrack`.
- Streaming time-lapse frame indexing with internal record, sequence,
  RIFF/WebP boundary, and observed VP8 dimension validation.
- Verified time-lapse full key-frame and positioned delta-patch helpers.
- Validated animation `FirstTrack` / `TrackNextIndex` chains and helpers for
  observed folder, static-image, paper, play-time, and audio track kinds.
- Sparse double-precision `FCurve` decoding for validated secondary
  action-mixer value records.
- Raster-state helpers for distinguishing default-filled images from decoded
  external block data.
- Common opaque block-status reporting through `BlockData::uniform_status`
  when every block in an external object agrees.
- Bounded standalone `.cmc` page-management loading with validated node trees,
  raw link preservation, and traversal-safe page-path resolution.
- Bounded decoding of all nine observed correction-layer kinds, including
  fixed-point raw values, gradient curves, and opaque future-kind fallback.
- Validated vector-ruler references and typed, bounded metadata for all nine
  observed special-ruler table kinds and perspective vanishing-point chains.
- Validated 2D-camera layer snapshots, typed kind-`2005` saved-position values,
  and axis-qualified primary and secondary position/center curves.
- Verified 2D-camera rotation in degrees and scale/opacity percentages.
- Secondary-curve decoding across observed metadata-header variants, including
  double-precision `PlayTime` records.

### Documentation

- Clarify that observed time-lapse streams encode frame order but no
  recoverable wall-clock timestamp.

## [0.2.0] - 2026-07-23

### Added

- Direct resolution of a layer-mask raster source through `Database::layer_mask_raster_source`.
- Bounded `CanvasPreview` extraction with PNG IHDR dimension validation.
- Bounded raw access to external vector-layer data through
  `Database::vector_data_sources` and `ClipFile::read_vector_data`.
- Bounded read and streaming copy APIs for complete external-object bodies.
- Bounded UTF-8 text-layer extraction with opaque attribute preservation and
  checked parsing of additional-object arrays.
- Optional timeline and image-cel selection decoding, including bounded BINC
  mixer parsing and layer-UUID resolution.
- Generic primary action-mixer `FCurve` access with raw track kinds,
  interpolation, slopes, optional tags, and audio/play-time curve support.
- Optional validated time-lapse manager, record, and blob chains with bounded
  allocation and streaming zlib decoding.
- Indexed external-object resolution through `ClipFile::resolve_external_object`.

### Documentation

- Record real-file validation of 8-bit grayscale layer masks and opaque vector
  references, UTF-8 text storage, and image-cel animation curves; anonymize
  local-corpus reporting.

## [0.1.0] - 2026-07-22

### Added

- Initial crate and project infrastructure.
- Streaming parser for the CLIP top-level container.
- Structural validation and an `inspect` example.
- Configurable parsing safety limits.
- External-body classification and block-data indexing.
- Optional bundled-SQLite access, schema discovery, and external-index validation.
- Optional offscreen metadata parsing, bounded zlib tile decoding, and raster assembly.
- Project, canvas, layer, and cycle-checked layer-tree models.

[Unreleased]: https://github.com/Aodaruma/clipfile-rs/compare/v1.0.0-rc.1...HEAD
[1.0.0-rc.1]: https://github.com/Aodaruma/clipfile-rs/compare/v0.5.0...v1.0.0-rc.1
[0.5.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Aodaruma/clipfile-rs/releases/tag/v0.1.0
