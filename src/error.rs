use std::{error, fmt, io};

/// An error encountered while reading or validating a CLIP container.
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
            Self::UnexpectedChunk { expected, actual } => {
                write!(formatter, "expected {expected}, found {actual:?}")
            }
            Self::PayloadTooLarge { size, limit } => {
                write!(formatter, "payload size {size} exceeds limit {limit}")
            }
            Self::InvalidChunkSequence { reason } => {
                write!(formatter, "invalid top-level chunk sequence: {reason}")
            }
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The result type used by this crate.
pub type Result<T> = std::result::Result<T, Error>;
