//! Validated, forward-compatible access to CLIP STUDIO PAINT files.
//!
//! The on-disk format is proprietary and not officially documented. This
//! crate focuses on the portions that can be validated consistently: the
//! `CSFCHUNK` envelope, top-level chunks, the file header, and external chunk
//! headers. Optional features add typed document, image, animation, time-lapse,
//! and conservative rewriting APIs as those parts become sufficiently well
//! understood. Low-level container, SQLite, raw-value, and opaque-body access
//! remains available as an escape hatch for future or not-yet-understood data.

#[cfg(feature = "animation")]
mod animation;
#[cfg(feature = "sqlite")]
mod cmc;
mod container;
#[cfg(feature = "sqlite")]
mod correction;
#[cfg(feature = "sqlite")]
mod database;
mod error;
mod external;
mod limits;
#[cfg(feature = "sqlite")]
mod model;
#[cfg(any(feature = "raster", feature = "write"))]
mod raster;
#[cfg(feature = "sqlite")]
mod ruler;
#[cfg(feature = "sqlite")]
mod text;
#[cfg(feature = "timelapse")]
mod timelapse;
#[cfg(feature = "sqlite")]
mod vector;
#[cfg(feature = "write")]
mod writer;

#[cfg(feature = "animation")]
pub use animation::{
    Animation, AnimationCurve, AnimationCurveKeyframe, AnimationCurveKeyframeValues,
    AnimationTrack, AnimationTrackKind, AnimationTrackValue, AnimationTrackValueEntry,
    Camera2DLayerData, Camera2DPoint, Camera2DTrackValues, Camera2DTransform, CelKeyframe,
    CelTrack, SecondaryAnimationCurve, SecondaryAnimationCurveKeyframe, Timeline,
};
#[cfg(all(feature = "animation", feature = "write"))]
pub use animation::{
    AnimationCurveKeyframeInsert, AnimationTrackCloneSummary, AnimationTrackRemovalSummary,
    ImageCelTrackCloneOptions, ImageCelTrackKeyframe,
};
#[cfg(feature = "sqlite")]
pub use cmc::{CmcFile, CmcNode};
pub use container::{
    CHUNK_HEADER_SIZE, ChunkHeader, ChunkIter, ChunkKind, ClipFile, ExternalChunkHeader,
    FileHeader, ROOT_HEADER_SIZE, RootHeader, ValidationSummary,
};
#[cfg(feature = "sqlite")]
pub use correction::{
    ColorBalanceAdjustment, Correction, CorrectionCurve, CorrectionCurvePoint,
    CorrectionGradientPoint, CorrectionGradientStop, CorrectionLayerData, CorrectionLevel,
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
    DecodedTile, Gray8Pixel, Gray8PixelMut, Gray8Pixels, Gray8PixelsMut, GrayAlpha8Pixel,
    GrayAlpha8PixelMut, GrayAlpha8Pixels, GrayAlpha8PixelsMut, OffscreenAttributes, PixelFormat,
    PixelPacking, RasterDataState, RasterImage, RasterPixel, RasterPixelMut, RasterPixels,
    RasterPixelsMut, RasterSource, Rgba8Pixel, Rgba8PixelMut, Rgba8Pixels, Rgba8PixelsMut,
};
#[cfg(feature = "sqlite")]
pub use ruler::{
    Ruler, RulerCurveData, RulerCurvePoint, RulerKind, RulerLayerData, RulerPoint, RulerVanishPoint,
};
#[cfg(feature = "write")]
pub use text::TextObjectWriteSummary;
#[cfg(feature = "sqlite")]
pub use text::{TextLayerData, TextObjectData};
#[cfg(feature = "timelapse")]
pub use timelapse::{
    TimeLapse, TimeLapseBlob, TimeLapseFrame, TimeLapseFrameKind, TimeLapseManager, TimeLapseRecord,
};
#[cfg(feature = "sqlite")]
pub use vector::VectorDataSource;
#[cfg(feature = "write")]
pub use vector::VectorTranslationSummary;
#[cfg(all(feature = "write", feature = "raster"))]
pub use writer::RasterWriteSummary;
#[cfg(feature = "write")]
pub use writer::{
    BlockChecksumMode, BlockWriteSummary, ClipWriter, EditableDatabase, WriteSummary,
};
