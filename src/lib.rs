//! Low-level, streaming access to CLIP STUDIO PAINT container files.
//!
//! The on-disk format is proprietary and not officially documented. This
//! crate starts with the portions that can be validated consistently: the
//! `CSFCHUNK` envelope, top-level chunks, the file header, and external chunk
//! headers. Higher-level document and image APIs will be added as those parts
//! of the format become sufficiently well understood.

mod container;
mod error;
mod external;
mod limits;

pub use container::{
    CHUNK_HEADER_SIZE, ChunkHeader, ChunkIter, ChunkKind, ClipFile, ExternalChunkHeader,
    FileHeader, ROOT_HEADER_SIZE, RootHeader, ValidationSummary,
};
pub use error::{Error, Result};
pub use external::{
    Block, BlockData, BlockParameters, BlockPayload, ByteOrder, ExternalBody, ExternalObject,
    LengthPrefixedZlib, MediaKind,
};
pub use limits::Limits;
