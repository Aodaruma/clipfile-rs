use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::{Error, Limits, Result};

const ROOT_MAGIC: [u8; 8] = *b"CSFCHUNK";
const FILE_HEADER_TAG: [u8; 8] = *b"CHNKHead";
const EXTERNAL_TAG: [u8; 8] = *b"CHNKExta";
const SQLITE_TAG: [u8; 8] = *b"CHNKSQLi";
const FOOTER_TAG: [u8; 8] = *b"CHNKFoot";

/// Size of the root container header in bytes.
pub const ROOT_HEADER_SIZE: u64 = 24;
/// Size of a top-level chunk header in bytes.
pub const CHUNK_HEADER_SIZE: u64 = 16;

/// The root `CSFCHUNK` header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootHeader {
    declared_file_size: u64,
    first_chunk_offset: u64,
}

impl RootHeader {
    /// File size recorded in the container header.
    #[must_use]
    pub const fn declared_file_size(&self) -> u64 {
        self.declared_file_size
    }

    /// Absolute offset of the first top-level chunk.
    #[must_use]
    pub const fn first_chunk_offset(&self) -> u64 {
        self.first_chunk_offset
    }
}

/// The known kind of a top-level chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChunkKind {
    /// The file metadata header (`CHNKHead`).
    Header,
    /// An external data object (`CHNKExta`).
    External,
    /// The embedded SQLite database (`CHNKSQLi`).
    Sqlite,
    /// The empty terminal chunk (`CHNKFoot`).
    Footer,
    /// A well-formed but currently unknown `CHNKxxxx` tag.
    Other([u8; 8]),
}

impl From<[u8; 8]> for ChunkKind {
    fn from(tag: [u8; 8]) -> Self {
        match tag {
            FILE_HEADER_TAG => Self::Header,
            EXTERNAL_TAG => Self::External,
            SQLITE_TAG => Self::Sqlite,
            FOOTER_TAG => Self::Footer,
            other => Self::Other(other),
        }
    }
}

/// Metadata for one top-level chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkHeader {
    tag: [u8; 8],
    offset: u64,
    payload_offset: u64,
    payload_size: u64,
}

impl ChunkHeader {
    /// The raw 8-byte chunk tag.
    #[must_use]
    pub const fn tag(&self) -> [u8; 8] {
        self.tag
    }

    /// The recognized chunk kind.
    #[must_use]
    pub fn kind(&self) -> ChunkKind {
        self.tag.into()
    }

    /// Absolute offset of the 16-byte chunk header.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Absolute offset of the chunk payload.
    #[must_use]
    pub const fn payload_offset(&self) -> u64 {
        self.payload_offset
    }

    /// Length of the chunk payload.
    #[must_use]
    pub const fn payload_size(&self) -> u64 {
        self.payload_size
    }

    fn end_offset(&self) -> Result<u64> {
        self.payload_offset
            .checked_add(self.payload_size)
            .ok_or(Error::OffsetOverflow)
    }
}

/// Parsed fields from the `CHNKHead` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileHeader {
    format_version: u64,
    database_offset: u64,
    identifier: Box<[u8]>,
}

impl FileHeader {
    /// Container format value. All currently analyzed samples use `256`.
    #[must_use]
    pub const fn format_version(&self) -> u64 {
        self.format_version
    }

    /// Absolute offset of the `CHNKSQLi` chunk header.
    #[must_use]
    pub const fn database_offset(&self) -> u64 {
        self.database_offset
    }

    /// Opaque file identifier bytes. Current samples contain a 16-byte UUID.
    #[must_use]
    pub fn identifier(&self) -> &[u8] {
        &self.identifier
    }
}

/// Parsed prefix of a `CHNKExta` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalChunkHeader {
    pub(crate) identifier: Box<[u8]>,
    pub(crate) body_offset: u64,
    pub(crate) body_size: u64,
}

impl ExternalChunkHeader {
    /// Opaque external object identifier.
    #[must_use]
    pub fn identifier(&self) -> &[u8] {
        &self.identifier
    }

    /// Absolute offset at which the external object's body begins.
    #[must_use]
    pub const fn body_offset(&self) -> u64 {
        self.body_offset
    }

    /// Length of the external object's body.
    #[must_use]
    pub const fn body_size(&self) -> u64 {
        self.body_size
    }
}

/// Counts returned after strict top-level validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationSummary {
    external_chunks: u64,
    database_payload_size: u64,
}

impl ValidationSummary {
    /// Number of external data chunks.
    #[must_use]
    pub const fn external_chunks(&self) -> u64 {
        self.external_chunks
    }

    /// Length of the embedded SQLite database.
    #[must_use]
    pub const fn database_payload_size(&self) -> u64 {
        self.database_payload_size
    }
}

/// A seekable CLIP container reader.
///
/// Opening validates the root header and parses `CHNKHead`. Large payloads are
/// not loaded into memory. Use [`ClipFile::chunks`] to scan metadata and
/// [`ClipFile::copy_chunk_payload`] to stream an individual payload.
pub struct ClipFile<R> {
    pub(crate) reader: R,
    root_header: RootHeader,
    file_header: FileHeader,
    pub(crate) file_size: u64,
    pub(crate) limits: Limits,
}

impl<R: Read + Seek> ClipFile<R> {
    /// Opens and validates the root and file headers of a seekable stream.
    pub fn open(reader: R) -> Result<Self> {
        Self::open_with_limits(reader, Limits::default())
    }

    /// Opens a seekable stream with custom parser safety limits.
    pub fn open_with_limits(mut reader: R, limits: Limits) -> Result<Self> {
        let actual_size = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;

        let magic = read_array::<8, _>(&mut reader)?;
        if magic != ROOT_MAGIC {
            return Err(Error::InvalidMagic(magic));
        }
        let declared_file_size = read_u64_be(&mut reader)?;
        let first_chunk_offset = read_u64_be(&mut reader)?;
        if declared_file_size != actual_size {
            return Err(Error::FileSizeMismatch {
                declared: declared_file_size,
                actual: actual_size,
            });
        }
        if first_chunk_offset < ROOT_HEADER_SIZE
            || first_chunk_offset
                .checked_add(CHUNK_HEADER_SIZE)
                .is_none_or(|minimum_end| minimum_end > actual_size)
        {
            return Err(Error::InvalidFirstChunkOffset {
                offset: first_chunk_offset,
                file_size: actual_size,
            });
        }
        let root_header = RootHeader {
            declared_file_size,
            first_chunk_offset,
        };

        let header_chunk = read_chunk_header(&mut reader, first_chunk_offset, actual_size)?;
        if header_chunk.tag != FILE_HEADER_TAG {
            return Err(Error::MissingFileHeader);
        }
        let file_header =
            read_file_header(&mut reader, &header_chunk, limits.max_identifier_size())?;

        Ok(Self {
            reader,
            root_header,
            file_header,
            file_size: actual_size,
            limits,
        })
    }

    /// Returns the root container header.
    #[must_use]
    pub const fn root_header(&self) -> &RootHeader {
        &self.root_header
    }

    /// Returns the parsed `CHNKHead` payload.
    #[must_use]
    pub const fn file_header(&self) -> &FileHeader {
        &self.file_header
    }

    /// Returns the active parser safety limits.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// Returns an iterator over top-level chunk headers.
    pub fn chunks(&mut self) -> ChunkIter<'_, R> {
        ChunkIter {
            reader: &mut self.reader,
            next_offset: Some(self.root_header.first_chunk_offset),
            file_size: self.file_size,
            chunks_seen: 0,
            max_chunks: self.limits.max_top_level_chunks(),
        }
    }

    #[cfg(feature = "raster")]
    pub(crate) fn chunk_at_offset(&mut self, offset: u64) -> Result<ChunkHeader> {
        read_chunk_header(&mut self.reader, offset, self.file_size)
    }

    /// Parses the internal prefix of an external chunk.
    pub fn external_chunk_header(&mut self, chunk: &ChunkHeader) -> Result<ExternalChunkHeader> {
        if chunk.kind() != ChunkKind::External {
            return Err(Error::UnexpectedChunk {
                expected: "CHNKExta",
                actual: chunk.tag,
            });
        }
        self.reader.seek(SeekFrom::Start(chunk.payload_offset))?;
        let identifier_size = read_u64_be(&mut self.reader)?;
        if identifier_size > self.limits.max_identifier_size() {
            return Err(Error::InvalidExternalChunk {
                reason: "external identifier exceeds the safety limit",
            });
        }
        let prefix_size = 16_u64
            .checked_add(identifier_size)
            .ok_or(Error::OffsetOverflow)?;
        if prefix_size > chunk.payload_size {
            return Err(Error::InvalidExternalChunk {
                reason: "external identifier exceeds chunk payload",
            });
        }
        let identifier = read_boxed(&mut self.reader, identifier_size)?;
        let body_size = read_u64_be(&mut self.reader)?;
        if prefix_size
            .checked_add(body_size)
            .ok_or(Error::OffsetOverflow)?
            != chunk.payload_size
        {
            return Err(Error::InvalidExternalChunk {
                reason: "external body size does not match chunk payload",
            });
        }
        let body_offset = chunk
            .payload_offset
            .checked_add(prefix_size)
            .ok_or(Error::OffsetOverflow)?;
        Ok(ExternalChunkHeader {
            identifier,
            body_offset,
            body_size,
        })
    }

    /// Reads a complete chunk payload after enforcing an allocation limit.
    pub fn read_chunk_payload(&mut self, chunk: &ChunkHeader, limit: u64) -> Result<Vec<u8>> {
        if chunk.payload_size > limit {
            return Err(Error::PayloadTooLarge {
                size: chunk.payload_size,
                limit,
            });
        }
        self.reader.seek(SeekFrom::Start(chunk.payload_offset))?;
        let mut data = Vec::new();
        let capacity = usize::try_from(chunk.payload_size).map_err(|_| Error::PayloadTooLarge {
            size: chunk.payload_size,
            limit,
        })?;
        data.try_reserve_exact(capacity)
            .map_err(|_| Error::PayloadTooLarge {
                size: chunk.payload_size,
                limit,
            })?;
        self.reader
            .by_ref()
            .take(chunk.payload_size)
            .read_to_end(&mut data)?;
        if data.len() as u64 != chunk.payload_size {
            return Err(Error::ChunkOutOfBounds {
                offset: chunk.offset,
                payload_size: chunk.payload_size,
                file_size: self.file_size,
            });
        }
        Ok(data)
    }

    /// Streams a complete chunk payload into a writer.
    pub fn copy_chunk_payload<W: Write>(
        &mut self,
        chunk: &ChunkHeader,
        writer: &mut W,
    ) -> Result<u64> {
        self.reader.seek(SeekFrom::Start(chunk.payload_offset))?;
        let copied = io::copy(&mut self.reader.by_ref().take(chunk.payload_size), writer)?;
        if copied != chunk.payload_size {
            return Err(Error::ChunkOutOfBounds {
                offset: chunk.offset,
                payload_size: chunk.payload_size,
                file_size: self.file_size,
            });
        }
        Ok(copied)
    }

    /// Strictly validates the known top-level sequence.
    ///
    /// The accepted order is one header, zero or more external chunks, one
    /// SQLite chunk at the offset declared by `CHNKHead`, and one empty footer.
    pub fn validate(&mut self) -> Result<ValidationSummary> {
        let database_offset = self.file_header.database_offset;
        let file_size = self.file_size;
        let max_database_size = self.limits.max_database_size();
        let mut chunks = self.chunks();
        let first = chunks.next().transpose()?.ok_or(Error::MissingFileHeader)?;
        if first.kind() != ChunkKind::Header {
            return Err(Error::MissingFileHeader);
        }

        let mut external_chunks = 0_u64;
        let mut database_payload_size = None;
        let mut saw_footer = false;
        for chunk in chunks {
            let chunk = chunk?;
            match chunk.kind() {
                ChunkKind::External if database_payload_size.is_none() => {
                    external_chunks = external_chunks
                        .checked_add(1)
                        .ok_or(Error::OffsetOverflow)?;
                }
                ChunkKind::Sqlite if database_payload_size.is_none() => {
                    if chunk.payload_size > max_database_size {
                        return Err(Error::LimitExceeded {
                            resource: "SQLite payload size",
                            value: chunk.payload_size,
                            limit: max_database_size,
                        });
                    }
                    if chunk.offset != database_offset {
                        return Err(Error::InvalidChunkSequence {
                            reason: "SQLite offset does not match CHNKHead",
                        });
                    }
                    database_payload_size = Some(chunk.payload_size);
                }
                ChunkKind::Footer if database_payload_size.is_some() && !saw_footer => {
                    if chunk.payload_size != 0 || chunk.end_offset()? != file_size {
                        return Err(Error::InvalidChunkSequence {
                            reason: "footer must be empty and terminate the file",
                        });
                    }
                    saw_footer = true;
                }
                _ => {
                    return Err(Error::InvalidChunkSequence {
                        reason: "unexpected chunk kind or order",
                    });
                }
            }
        }
        if !saw_footer {
            return Err(Error::InvalidChunkSequence {
                reason: "missing SQLite chunk or footer",
            });
        }
        Ok(ValidationSummary {
            external_chunks,
            database_payload_size: database_payload_size.expect("footer requires database"),
        })
    }

    /// Returns the wrapped reader.
    #[must_use]
    pub fn into_inner(self) -> R {
        self.reader
    }
}

/// Iterator over top-level chunk headers.
pub struct ChunkIter<'a, R> {
    reader: &'a mut R,
    next_offset: Option<u64>,
    file_size: u64,
    chunks_seen: u64,
    max_chunks: u64,
}

impl<R: Read + Seek> Iterator for ChunkIter<'_, R> {
    type Item = Result<ChunkHeader>;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.next_offset?;
        if offset == self.file_size {
            self.next_offset = None;
            return None;
        }
        if self.chunks_seen >= self.max_chunks {
            self.next_offset = None;
            return Some(Err(Error::LimitExceeded {
                resource: "top-level chunk count",
                value: self.chunks_seen.saturating_add(1),
                limit: self.max_chunks,
            }));
        }
        match read_chunk_header(self.reader, offset, self.file_size) {
            Ok(chunk) => match chunk.end_offset() {
                Ok(end_offset) => {
                    self.next_offset = Some(end_offset);
                    self.chunks_seen += 1;
                    Some(Ok(chunk))
                }
                Err(error) => {
                    self.next_offset = None;
                    Some(Err(error))
                }
            },
            Err(error) => {
                self.next_offset = None;
                Some(Err(error))
            }
        }
    }
}

fn read_chunk_header<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    file_size: u64,
) -> Result<ChunkHeader> {
    let header_end = offset
        .checked_add(CHUNK_HEADER_SIZE)
        .ok_or(Error::OffsetOverflow)?;
    if header_end > file_size {
        return Err(Error::ChunkOutOfBounds {
            offset,
            payload_size: 0,
            file_size,
        });
    }
    reader.seek(SeekFrom::Start(offset))?;
    let tag = read_array::<8, _>(reader)?;
    if &tag[..4] != b"CHNK" {
        return Err(Error::InvalidChunkTag { offset, tag });
    }
    let payload_size = read_u64_be(reader)?;
    let payload_end = header_end
        .checked_add(payload_size)
        .ok_or(Error::OffsetOverflow)?;
    if payload_end > file_size {
        return Err(Error::ChunkOutOfBounds {
            offset,
            payload_size,
            file_size,
        });
    }
    Ok(ChunkHeader {
        tag,
        offset,
        payload_offset: header_end,
        payload_size,
    })
}

fn read_file_header<R: Read + Seek>(
    reader: &mut R,
    chunk: &ChunkHeader,
    max_identifier_size: u64,
) -> Result<FileHeader> {
    if chunk.payload_size < 24 {
        return Err(Error::InvalidFileHeader {
            reason: "payload is shorter than the fixed fields",
        });
    }
    reader.seek(SeekFrom::Start(chunk.payload_offset))?;
    let format_version = read_u64_be(reader)?;
    let database_offset = read_u64_be(reader)?;
    let identifier_size = read_u64_be(reader)?;
    if identifier_size > max_identifier_size {
        return Err(Error::InvalidFileHeader {
            reason: "identifier exceeds the safety limit",
        });
    }
    let expected_size = 24_u64
        .checked_add(identifier_size)
        .ok_or(Error::OffsetOverflow)?;
    if expected_size != chunk.payload_size {
        return Err(Error::InvalidFileHeader {
            reason: "identifier length does not match payload size",
        });
    }
    let identifier = read_boxed(reader, identifier_size)?;
    Ok(FileHeader {
        format_version,
        database_offset,
        identifier,
    })
}

fn read_u64_be<R: Read>(reader: &mut R) -> Result<u64> {
    Ok(u64::from_be_bytes(read_array::<8, _>(reader)?))
}

fn read_array<const N: usize, R: Read>(reader: &mut R) -> Result<[u8; N]> {
    let mut data = [0_u8; N];
    reader.read_exact(&mut data)?;
    Ok(data)
}

fn read_boxed<R: Read>(reader: &mut R, size: u64) -> Result<Box<[u8]>> {
    let size = usize::try_from(size).map_err(|_| Error::InvalidFileHeader {
        reason: "byte string is too large for this platform",
    })?;
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| Error::InvalidFileHeader {
            reason: "byte string allocation failed",
        })?;
    data.resize(size, 0);
    reader.read_exact(&mut data)?;
    Ok(data.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_chunk(bytes: &mut Vec<u8>, tag: &[u8; 8], payload: &[u8]) -> u64 {
        let offset = bytes.len() as u64;
        bytes.extend_from_slice(tag);
        push_u64(bytes, payload.len() as u64);
        bytes.extend_from_slice(payload);
        offset
    }

    fn sample(with_external: bool) -> Vec<u8> {
        let mut bytes = Vec::from(ROOT_MAGIC);
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, ROOT_HEADER_SIZE);

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        let database_offset_position = header.len();
        push_u64(&mut header, 0);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);
        push_chunk(&mut bytes, &FILE_HEADER_TAG, &header);

        if with_external {
            let identifier = b"extrnlid0123456789ABCDEF0123456789ABCDEF";
            let mut external = Vec::new();
            push_u64(&mut external, identifier.len() as u64);
            external.extend_from_slice(identifier);
            push_u64(&mut external, 3);
            external.extend_from_slice(b"abc");
            push_chunk(&mut bytes, &EXTERNAL_TAG, &external);
        }

        let database_offset = push_chunk(&mut bytes, &SQLITE_TAG, b"db!");
        push_chunk(&mut bytes, &FOOTER_TAG, b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        let absolute_database_field =
            ROOT_HEADER_SIZE as usize + CHUNK_HEADER_SIZE as usize + database_offset_position;
        bytes[absolute_database_field..absolute_database_field + 8]
            .copy_from_slice(&database_offset.to_be_bytes());
        bytes
    }

    #[test]
    fn opens_and_validates_minimal_container() {
        let bytes = sample(false);
        let mut file = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert_eq!(file.file_header().format_version(), 256);
        assert_eq!(file.file_header().identifier(), &[0x42; 16]);
        assert_eq!(
            file.validate().unwrap(),
            ValidationSummary {
                external_chunks: 0,
                database_payload_size: 3,
            }
        );
    }

    #[test]
    fn scans_and_parses_external_header() {
        let bytes = sample(true);
        let mut file = ClipFile::open(Cursor::new(bytes)).unwrap();
        let chunks = file.chunks().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(chunks.len(), 4);
        let external = file.external_chunk_header(&chunks[1]).unwrap();
        assert_eq!(
            external.identifier(),
            b"extrnlid0123456789ABCDEF0123456789ABCDEF"
        );
        assert_eq!(external.body_size(), 3);
        assert_eq!(file.validate().unwrap().external_chunks(), 1);
    }

    #[test]
    fn enforces_payload_allocation_limit() {
        let bytes = sample(false);
        let mut file = ClipFile::open(Cursor::new(bytes)).unwrap();
        let database = file
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::Sqlite).then_some(chunk)
            })
            .unwrap();
        assert!(matches!(
            file.read_chunk_payload(&database, 2),
            Err(Error::PayloadTooLarge { size: 3, limit: 2 })
        ));
        assert_eq!(file.read_chunk_payload(&database, 3).unwrap(), b"db!");
    }

    #[test]
    fn rejects_wrong_magic_and_size() {
        let mut wrong_magic = sample(false);
        wrong_magic[0] = b'X';
        assert!(matches!(
            ClipFile::open(Cursor::new(wrong_magic)),
            Err(Error::InvalidMagic(_))
        ));

        let mut wrong_size = sample(false);
        wrong_size[15] ^= 1;
        assert!(matches!(
            ClipFile::open(Cursor::new(wrong_size)),
            Err(Error::FileSizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_out_of_bounds_header_chunk() {
        let mut bytes = sample(false);
        bytes[32..40].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(matches!(
            ClipFile::open(Cursor::new(bytes)),
            Err(Error::OffsetOverflow | Error::ChunkOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_mismatched_external_body_size() {
        let mut bytes = sample(true);
        let mut file = ClipFile::open(Cursor::new(bytes.clone())).unwrap();
        let external = file
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::External).then_some(chunk)
            })
            .unwrap();
        let body_size_offset = external.payload_offset() as usize + 8 + 40;
        bytes[body_size_offset..body_size_offset + 8].copy_from_slice(&4_u64.to_be_bytes());

        let mut file = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            file.external_chunk_header(&external),
            Err(Error::InvalidExternalChunk { .. })
        ));
    }
}
