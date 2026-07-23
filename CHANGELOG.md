# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Bounded decoding of inline animation `TrackValueMap` entries, including
  observed floating-point and indexed-text values with opaque fallback.
- Secondary action-mixer presence reporting on `AnimationTrack`.
- Streaming time-lapse frame indexing with internal record, sequence,
  RIFF/WebP boundary, and observed VP8 dimension validation.
- Validated animation `FirstTrack` / `TrackNextIndex` chains and helpers for
  observed folder, static-image, paper, play-time, and audio track kinds.
- Sparse double-precision `FCurve` decoding for validated secondary
  action-mixer value records.

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

[Unreleased]: https://github.com/Aodaruma/clipfile-rs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Aodaruma/clipfile-rs/releases/tag/v0.1.0
