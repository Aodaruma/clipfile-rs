use std::{error, fmt, io};

/// An error encountered while reading, writing, or validating a CLIP container.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An underlying I/O operation failed.
    Io(io::Error),
    /// The root `CSFCHUNK` signature was not present.
    InvalidMagic([u8; 8]),
    /// The declared size does not match the length of the input stream.
    FileSizeMismatch {
        /// Size recorded in the root header.
        declared: u64,
        /// Actual stream length.
        actual: u64,
    },
    /// The first chunk offset is outside the valid file range.
    InvalidFirstChunkOffset {
        /// Offset recorded in the root header.
        offset: u64,
        /// Actual stream length.
        file_size: u64,
    },
    /// A chunk tag does not begin with the expected `CHNK` prefix.
    InvalidChunkTag {
        /// Offset of the invalid tag.
        offset: u64,
        /// Bytes read from the file.
        tag: [u8; 8],
    },
    /// A chunk payload extends beyond the declared file boundary.
    ChunkOutOfBounds {
        /// Offset of the chunk header.
        offset: u64,
        /// Declared payload size.
        payload_size: u64,
        /// Declared file size.
        file_size: u64,
    },
    /// Integer arithmetic overflowed while calculating an offset.
    OffsetOverflow,
    /// The first top-level chunk was not `CHNKHead`.
    MissingFileHeader,
    /// The `CHNKHead` payload has an invalid internal layout.
    InvalidFileHeader {
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },
    /// A `CHNKExta` payload has an invalid internal layout.
    InvalidExternalChunk {
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },
    /// A block-data body has an invalid internal layout.
    InvalidBlockData {
        /// Absolute offset at which the problem was found.
        offset: u64,
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },
    /// A configurable parser safety limit was exceeded.
    LimitExceeded {
        /// Name of the limited resource.
        resource: &'static str,
        /// Value found in the input.
        value: u64,
        /// Configured maximum.
        limit: u64,
    },
    /// A method received a chunk of the wrong kind.
    UnexpectedChunk {
        /// Chunk required by the operation.
        expected: &'static str,
        /// Actual raw chunk tag.
        actual: [u8; 8],
    },
    /// A requested in-memory payload exceeds the caller-provided limit.
    PayloadTooLarge {
        /// Payload size in the file.
        size: u64,
        /// Maximum accepted size.
        limit: u64,
    },
    /// The top-level chunk sequence violates the currently known structure.
    InvalidChunkSequence {
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },
    /// The embedded SQLite database is structurally inconsistent.
    #[cfg(feature = "sqlite")]
    InvalidDatabase {
        /// Human-readable details.
        reason: String,
    },
    /// A requested SQLite table does not exist in this file revision.
    #[cfg(feature = "sqlite")]
    MissingTable {
        /// Requested table name.
        table: String,
    },
    /// A requested SQLite column does not exist in this file revision.
    #[cfg(feature = "sqlite")]
    MissingColumn {
        /// Table containing the requested column.
        table: String,
        /// Requested column name.
        column: String,
    },
    /// An operation on the embedded SQLite database failed.
    #[cfg(feature = "sqlite")]
    Sqlite(rusqlite::Error),
    /// The high-level document model is internally inconsistent.
    #[cfg(feature = "sqlite")]
    InvalidDocument {
        /// Human-readable details.
        reason: String,
    },
    /// A standalone `.cmc` page-management database is inconsistent.
    #[cfg(feature = "sqlite")]
    InvalidCmc {
        /// Human-readable details.
        reason: String,
    },
    /// Correction-layer metadata is structurally inconsistent.
    #[cfg(feature = "sqlite")]
    InvalidCorrection {
        /// Human-readable details.
        reason: String,
    },
    /// Ruler metadata or its linked lists are structurally inconsistent.
    #[cfg(feature = "sqlite")]
    InvalidRuler {
        /// Human-readable details.
        reason: String,
    },
    /// Raster metadata or decoded pixels are structurally inconsistent.
    #[cfg(any(feature = "raster", feature = "write"))]
    InvalidRaster {
        /// Human-readable details.
        reason: String,
    },
    /// The raster uses a pixel layout that is recognized but not supported.
    #[cfg(any(feature = "raster", feature = "write"))]
    UnsupportedRaster {
        /// Human-readable details.
        reason: String,
    },
    /// Animation metadata or mixer data is structurally inconsistent.
    #[cfg(feature = "animation")]
    InvalidAnimation {
        /// Human-readable details.
        reason: String,
    },
    /// Time-lapse metadata or payload data is structurally inconsistent.
    #[cfg(feature = "timelapse")]
    InvalidTimeLapse {
        /// Human-readable details.
        reason: String,
    },
    /// A requested CLIP rewrite would violate a validated invariant.
    #[cfg(feature = "write")]
    InvalidWrite {
        /// Human-readable details.
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidMagic(magic) => {
                write!(formatter, "invalid CLIP magic: {magic:?}")
            }
            Self::FileSizeMismatch { declared, actual } => write!(
                formatter,
                "declared file size {declared} does not match actual size {actual}"
            ),
            Self::InvalidFirstChunkOffset { offset, file_size } => write!(
                formatter,
                "first chunk offset {offset} is invalid for a {file_size}-byte file"
            ),
            Self::InvalidChunkTag { offset, tag } => {
                write!(formatter, "invalid chunk tag {tag:?} at offset {offset}")
            }
            Self::ChunkOutOfBounds {
                offset,
                payload_size,
                file_size,
            } => write!(
                formatter,
                "chunk at offset {offset} with {payload_size}-byte payload exceeds file size {file_size}"
            ),
            Self::OffsetOverflow => formatter.write_str("offset arithmetic overflow"),
            Self::MissingFileHeader => formatter.write_str("first chunk is not CHNKHead"),
            Self::InvalidFileHeader { reason } => {
                write!(formatter, "invalid CHNKHead payload: {reason}")
            }
            Self::InvalidExternalChunk { reason } => {
                write!(formatter, "invalid CHNKExta payload: {reason}")
            }
            Self::InvalidBlockData { offset, reason } => {
                write!(formatter, "invalid block data at offset {offset}: {reason}")
            }
            Self::LimitExceeded {
                resource,
                value,
                limit,
            } => write!(
                formatter,
                "{resource} value {value} exceeds configured limit {limit}"
            ),
            Self::UnexpectedChunk { expected, actual } => {
                write!(formatter, "expected {expected}, found {actual:?}")
            }
            Self::PayloadTooLarge { size, limit } => {
                write!(formatter, "payload size {size} exceeds limit {limit}")
            }
            Self::InvalidChunkSequence { reason } => {
                write!(formatter, "invalid top-level chunk sequence: {reason}")
            }
            #[cfg(feature = "sqlite")]
            Self::InvalidDatabase { reason } => {
                write!(formatter, "invalid embedded SQLite database: {reason}")
            }
            #[cfg(feature = "sqlite")]
            Self::MissingTable { table } => {
                write!(formatter, "SQLite table {table:?} is not present")
            }
            #[cfg(feature = "sqlite")]
            Self::MissingColumn { table, column } => {
                write!(formatter, "SQLite column {table}.{column} is not present")
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            #[cfg(feature = "sqlite")]
            Self::InvalidDocument { reason } => {
                write!(formatter, "invalid CLIP document model: {reason}")
            }
            #[cfg(feature = "sqlite")]
            Self::InvalidCmc { reason } => {
                write!(formatter, "invalid CLIP page-management file: {reason}")
            }
            #[cfg(feature = "sqlite")]
            Self::InvalidCorrection { reason } => {
                write!(formatter, "invalid correction-layer data: {reason}")
            }
            #[cfg(feature = "sqlite")]
            Self::InvalidRuler { reason } => {
                write!(formatter, "invalid ruler data: {reason}")
            }
            #[cfg(any(feature = "raster", feature = "write"))]
            Self::InvalidRaster { reason } => write!(formatter, "invalid raster data: {reason}"),
            #[cfg(any(feature = "raster", feature = "write"))]
            Self::UnsupportedRaster { reason } => {
                write!(formatter, "unsupported raster layout: {reason}")
            }
            #[cfg(feature = "animation")]
            Self::InvalidAnimation { reason } => {
                write!(formatter, "invalid animation data: {reason}")
            }
            #[cfg(feature = "timelapse")]
            Self::InvalidTimeLapse { reason } => {
                write!(formatter, "invalid time-lapse data: {reason}")
            }
            #[cfg(feature = "write")]
            Self::InvalidWrite { reason } => {
                write!(formatter, "cannot write CLIP container: {reason}")
            }
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// The result type used by this crate.
pub type Result<T> = std::result::Result<T, Error>;
