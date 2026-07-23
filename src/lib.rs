//! Low-level, streaming access to CLIP STUDIO PAINT container files.
//!
//! The on-disk format is proprietary and not officially documented. This
//! crate starts with the portions that can be validated consistently: the
//! `CSFCHUNK` envelope, top-level chunks, the file header, and external chunk
//! headers. Higher-level document and image APIs will be added as those parts
//! of the format become sufficiently well understood.

#[cfg(feature = "animation")]
mod animation;
mod container;
#[cfg(feature = "sqlite")]
mod database;
mod error;
mod external;
mod limits;
#[cfg(feature = "sqlite")]
mod model;
#[cfg(feature = "raster")]
mod raster;
#[cfg(feature = "sqlite")]
mod text;
#[cfg(feature = "timelapse")]
mod timelapse;
#[cfg(feature = "sqlite")]
mod vector;

#[cfg(feature = "animation")]
pub use animation::{
    Animation, AnimationCurve, AnimationCurveKeyframe, AnimationTrack, AnimationTrackKind,
    AnimationTrackValue, AnimationTrackValueEntry, CelKeyframe, CelTrack, Timeline,
};
pub use container::{
    CHUNK_HEADER_SIZE, ChunkHeader, ChunkIter, ChunkKind, ClipFile, ExternalChunkHeader,
    FileHeader, ROOT_HEADER_SIZE, RootHeader, ValidationSummary,
};
#[cfg(feature = "sqlite")]
pub use database::{
    ColumnSchema, Database, DatabaseSchema, ExternalChunkRecord, ExternalReferenceColumn,
    TableSchema,
};
pub use error::{Error, Result};
pub use external::{
    Block, BlockData, BlockParameters, BlockPayload, ByteOrder, ExternalBody, ExternalObject,
    LengthPrefixedZlib, MediaKind,
};
pub use limits::Limits;
#[cfg(feature = "sqlite")]
pub use model::{BlendMode, Canvas, CanvasPreview, Document, Layer, LayerKind, LayerTree, Project};
#[cfg(feature = "raster")]
pub use raster::{
    DecodedTile, OffscreenAttributes, PixelFormat, PixelPacking, RasterDataState, RasterImage,
    RasterSource,
};
#[cfg(feature = "sqlite")]
pub use text::{TextLayerData, TextObjectData};
#[cfg(feature = "timelapse")]
pub use timelapse::{
    TimeLapse, TimeLapseBlob, TimeLapseFrame, TimeLapseFrameKind, TimeLapseManager, TimeLapseRecord,
};
#[cfg(feature = "sqlite")]
pub use vector::VectorDataSource;
