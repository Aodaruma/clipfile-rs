use std::io::{Read, Seek, SeekFrom, Write};

#[cfg(feature = "write")]
use flate2::{Compression, write::ZlibEncoder};

use crate::{ChunkHeader, ClipFile, Error, ExternalChunkHeader, Result};

const BLOCK_BEGIN: &[u8] = b"\0B\0l\0o\0c\0k\0D\0a\0t\0a\0B\0e\0g\0i\0n\0C\0h\0u\0n\0k";
const BLOCK_END: &[u8] = b"\0B\0l\0o\0c\0k\0D\0a\0t\0a\0E\0n\0d\0C\0h\0u\0n\0k";
const BLOCK_STATUS: &[u8] = b"\0B\0l\0o\0c\0k\0S\0t\0a\0t\0u\0s";
const BLOCK_CHECKSUM: &[u8] = b"\0B\0l\0o\0c\0k\0C\0h\0e\0c\0k\0S\0u\0m";

/// Byte order used by a length prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteOrder {
    /// Most-significant byte first.
    BigEndian,
    /// Least-significant byte first.
    LittleEndian,
}

/// A recognized raw media payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MediaKind {
    /// A RIFF/WAVE audio stream.
    Wave,
    /// An MP3 stream, with or without an ID3 prefix.
    Mp3,
}

/// Location of a zlib stream preceded by its compressed size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LengthPrefixedZlib {
    byte_order: ByteOrder,
    compressed_offset: u64,
    compressed_size: u64,
}

impl LengthPrefixedZlib {
    /// Byte order used by the four-byte size prefix.
    #[must_use]
    pub const fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }

    /// Absolute offset of the zlib stream, after the size prefix.
    #[must_use]
    pub const fn compressed_offset(&self) -> u64 {
        self.compressed_offset
    }

    /// Compressed zlib stream length.
    #[must_use]
    pub const fn compressed_size(&self) -> u64 {
        self.compressed_size
    }
}

/// Classification of an external object's body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExternalBody {
    /// A tiled `BlockDataBeginChunk` container.
    BlockData,
    /// A length-prefixed zlib stream.
    LengthPrefixedZlib(LengthPrefixedZlib),
    /// A raw media stream.
    Media(MediaKind),
    /// A body whose format is not currently recognized.
    Unknown,
}

/// A parsed external-object header and its body classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalObject {
    header: ExternalChunkHeader,
    body: ExternalBody,
}

impl ExternalObject {
    /// Parsed `CHNKExta` prefix.
    #[must_use]
    pub const fn header(&self) -> &ExternalChunkHeader {
        &self.header
    }

    /// Recognized external body format.
    #[must_use]
    pub const fn body(&self) -> ExternalBody {
        self.body
    }
}

/// The twelve-byte parameter record stored in every observed block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockParameters {
    raw: [u8; 12],
}

impl BlockParameters {
    /// Returns the uninterpreted bytes for forward-compatible access.
    #[must_use]
    pub const fn raw(&self) -> [u8; 12] {
        self.raw
    }

    /// Observed channel-count field.
    #[must_use]
    pub const fn channel_count(&self) -> u16 {
        u16::from_be_bytes([self.raw[0], self.raw[1]])
    }

    /// Observed reserved field.
    #[must_use]
    pub const fn reserved(&self) -> u16 {
        u16::from_be_bytes([self.raw[2], self.raw[3]])
    }

    /// Observed tile-width field.
    #[must_use]
    pub const fn width(&self) -> u32 {
        u32::from_be_bytes([self.raw[4], self.raw[5], self.raw[6], self.raw[7]])
    }

    /// Observed tile-height field.
    #[must_use]
    pub const fn height(&self) -> u32 {
        u32::from_be_bytes([self.raw[8], self.raw[9], self.raw[10], self.raw[11]])
    }
}

/// Location and metadata of one compressed block payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockPayload {
    offset: u64,
    compressed_size: u64,
    prefix: Option<[u8; 2]>,
}

impl BlockPayload {
    /// Absolute offset of the compressed bytes.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Compressed payload size.
    #[must_use]
    pub const fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// First two compressed bytes, when present.
    #[must_use]
    pub const fn prefix(&self) -> Option<[u8; 2]> {
        self.prefix
    }

    /// Returns whether the prefix is a valid zlib CMF/FLG pair.
    #[must_use]
    pub fn has_zlib_header(&self) -> bool {
        self.prefix.is_some_and(is_zlib_header)
    }
}

/// Metadata for one tile block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    index: u32,
    parameters: BlockParameters,
    payload: Option<BlockPayload>,
    status: u32,
    checksum: u32,
}

impl Block {
    /// Block index within the tile grid.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Raw and interpreted block parameters.
    #[must_use]
    pub const fn parameters(&self) -> BlockParameters {
        self.parameters
    }

    /// Compressed data location, or `None` for an empty block.
    #[must_use]
    pub const fn payload(&self) -> Option<BlockPayload> {
        self.payload
    }

    /// Opaque value from the `BlockStatus` array.
    #[must_use]
    pub const fn status(&self) -> u32 {
        self.status
    }

    /// Opaque value from the `BlockCheckSum` array.
    #[must_use]
    pub const fn checksum(&self) -> u32 {
        self.checksum
    }
}

/// An indexed block-data body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockData {
    blocks: Vec<Block>,
}

impl BlockData {
    /// All blocks in on-disk order.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Returns the common opaque `BlockStatus` value when every block agrees.
    ///
    /// The observed files store a single value across every block in one
    /// external object, but the value's semantic meaning remains unknown.
    /// Empty or mixed-status objects return `None`.
    #[must_use]
    pub fn uniform_status(&self) -> Option<u32> {
        let first = self.blocks.first()?.status;
        self.blocks
            .iter()
            .all(|block| block.status == first)
            .then_some(first)
    }

    /// Number of blocks with compressed data.
    #[must_use]
    pub fn present_blocks(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| block.payload.is_some())
            .count()
    }

    /// Number of empty blocks.
    #[must_use]
    pub fn empty_blocks(&self) -> usize {
        self.blocks.len() - self.present_blocks()
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Parses an external prefix and classifies its body without loading it.
    pub fn inspect_external_chunk(&mut self, chunk: &ChunkHeader) -> Result<ExternalObject> {
        let header = self.external_chunk_header(chunk)?;
        let body = classify_body(&mut self.reader, &header)?;
        Ok(ExternalObject { header, body })
    }

    /// Reads an external object's complete body after enforcing an allocation limit.
    pub fn read_external_body(&mut self, object: &ExternalObject, limit: u64) -> Result<Vec<u8>> {
        let size = object.header.body_size();
        if size > limit {
            return Err(Error::PayloadTooLarge { size, limit });
        }
        read_range(
            &mut self.reader,
            self.file_size,
            object.header.body_offset(),
            size,
        )
    }

    /// Streams an external object's complete body into a writer.
    pub fn copy_external_body<W: Write>(
        &mut self,
        object: &ExternalObject,
        writer: &mut W,
    ) -> Result<u64> {
        copy_range(
            &mut self.reader,
            self.file_size,
            object.header.body_offset(),
            object.header.body_size(),
            writer,
        )
    }

    /// Indexes a block-data body without reading compressed tile contents.
    pub fn read_block_data(&mut self, object: &ExternalObject) -> Result<BlockData> {
        if object.body != ExternalBody::BlockData {
            return Err(Error::InvalidExternalChunk {
                reason: "external object is not block data",
            });
        }
        parse_block_data(
            &mut self.reader,
            &object.header,
            self.limits.max_blocks_per_external(),
        )
    }

    /// Reads one compressed block payload after enforcing an allocation limit.
    pub fn read_block_payload(&mut self, payload: BlockPayload, limit: u64) -> Result<Vec<u8>> {
        if payload.compressed_size > limit {
            return Err(Error::PayloadTooLarge {
                size: payload.compressed_size,
                limit,
            });
        }
        read_range(
            &mut self.reader,
            self.file_size,
            payload.offset,
            payload.compressed_size,
        )
    }

    /// Streams one compressed block payload into a writer.
    pub fn copy_block_payload<W: Write>(
        &mut self,
        payload: BlockPayload,
        writer: &mut W,
    ) -> Result<u64> {
        copy_range(
            &mut self.reader,
            self.file_size,
            payload.offset,
            payload.compressed_size,
            writer,
        )
    }

    /// Reads a classified length-prefixed zlib stream with a caller limit.
    pub fn read_length_prefixed_zlib(
        &mut self,
        stream: LengthPrefixedZlib,
        limit: u64,
    ) -> Result<Vec<u8>> {
        if stream.compressed_size > limit {
            return Err(Error::PayloadTooLarge {
                size: stream.compressed_size,
                limit,
            });
        }
        read_range(
            &mut self.reader,
            self.file_size,
            stream.compressed_offset,
            stream.compressed_size,
        )
    }
}

fn classify_body<R: Read + Seek>(
    reader: &mut R,
    header: &ExternalChunkHeader,
) -> Result<ExternalBody> {
    reader.seek(SeekFrom::Start(header.body_offset))?;
    let prefix_size = header.body_size.min(16) as usize;
    let mut prefix = [0_u8; 16];
    reader.read_exact(&mut prefix[..prefix_size])?;
    let prefix = &prefix[..prefix_size];

    if looks_like_block_data(prefix) {
        return Ok(ExternalBody::BlockData);
    }
    if header.body_size >= 6 {
        let compressed_size = header.body_size - 4;
        let size_prefix: [u8; 4] = prefix[..4]
            .try_into()
            .expect("prefix is at least six bytes");
        let zlib_prefix: [u8; 2] = prefix[4..6]
            .try_into()
            .expect("prefix is at least six bytes");
        if is_zlib_header(zlib_prefix) {
            let byte_order = if u64::from(u32::from_be_bytes(size_prefix)) == compressed_size {
                Some(ByteOrder::BigEndian)
            } else if u64::from(u32::from_le_bytes(size_prefix)) == compressed_size {
                Some(ByteOrder::LittleEndian)
            } else {
                None
            };
            if let Some(byte_order) = byte_order {
                return Ok(ExternalBody::LengthPrefixedZlib(LengthPrefixedZlib {
                    byte_order,
                    compressed_offset: header
                        .body_offset
                        .checked_add(4)
                        .ok_or(Error::OffsetOverflow)?,
                    compressed_size,
                }));
            }
        }
    }
    if prefix.len() >= 12 && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WAVE" {
        return Ok(ExternalBody::Media(MediaKind::Wave));
    }
    if prefix.starts_with(b"ID3")
        || prefix
            .get(..2)
            .is_some_and(|bytes| bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        return Ok(ExternalBody::Media(MediaKind::Mp3));
    }
    Ok(ExternalBody::Unknown)
}

fn looks_like_block_data(prefix: &[u8]) -> bool {
    prefix.len() >= 12 && &prefix[4..12] == b"\0\0\0\x13\0B\0l"
        || prefix.len() >= 8 && &prefix[..8] == b"\0\0\0\x0b\0B\0l"
}

fn is_zlib_header(prefix: [u8; 2]) -> bool {
    prefix[0] & 0x0f == 8 && u16::from_be_bytes(prefix) % 31 == 0
}

fn parse_block_data<R: Read + Seek>(
    reader: &mut R,
    header: &ExternalChunkHeader,
    max_blocks: u64,
) -> Result<BlockData> {
    let body_end = header
        .body_offset
        .checked_add(header.body_size)
        .ok_or(Error::OffsetOverflow)?;
    reader.seek(SeekFrom::Start(header.body_offset))?;
    let mut blocks = Vec::new();
    let mut statuses = None;
    let mut checksums = None;

    while reader.stream_position()? < body_end {
        let item_start = reader.stream_position()?;
        let size_or_label = read_u32_be_bounded(reader, body_end)?;
        match size_or_label {
            11 => {
                if statuses.is_some() {
                    return Err(block_error(item_start, "duplicate BlockStatus"));
                }
                expect_bytes(reader, body_end, BLOCK_STATUS, item_start)?;
                statuses = Some(read_trailer_values(
                    reader,
                    body_end,
                    blocks.len(),
                    item_start,
                )?);
            }
            13 => {
                if checksums.is_some() {
                    return Err(block_error(item_start, "duplicate BlockCheckSum"));
                }
                expect_bytes(reader, body_end, BLOCK_CHECKSUM, item_start)?;
                checksums = Some(read_trailer_values(
                    reader,
                    body_end,
                    blocks.len(),
                    item_start,
                )?);
            }
            block_size => {
                if statuses.is_some() || checksums.is_some() {
                    return Err(block_error(
                        item_start,
                        "block appears after trailer arrays",
                    ));
                }
                let count = u64::try_from(blocks.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(1);
                if count > max_blocks {
                    return Err(Error::LimitExceeded {
                        resource: "blocks per external object",
                        value: count,
                        limit: max_blocks,
                    });
                }
                blocks.push(read_block(reader, body_end, item_start, block_size)?);
            }
        }
    }

    let statuses = statuses.ok_or_else(|| block_error(body_end, "missing BlockStatus"))?;
    let checksums = checksums.ok_or_else(|| block_error(body_end, "missing BlockCheckSum"))?;
    for ((block, status), checksum) in blocks.iter_mut().zip(statuses).zip(checksums) {
        block.status = status;
        block.checksum = checksum;
    }
    Ok(BlockData { blocks })
}

fn read_block<R: Read + Seek>(
    reader: &mut R,
    body_end: u64,
    block_start: u64,
    block_size: u32,
) -> Result<Block> {
    let block_end = block_start
        .checked_add(u64::from(block_size))
        .ok_or(Error::OffsetOverflow)?;
    if block_end > body_end {
        return Err(block_error(
            block_start,
            "block extends beyond external body",
        ));
    }
    let begin_chars = read_u32_be_bounded(reader, block_end)?;
    if begin_chars != 19 {
        return Err(block_error(block_start, "invalid begin-label length"));
    }
    expect_bytes(reader, block_end, BLOCK_BEGIN, block_start)?;
    let index = read_u32_be_bounded(reader, block_end)?;
    let parameters = BlockParameters {
        raw: read_array_bounded(reader, block_end)?,
    };
    let present = read_u32_be_bounded(reader, block_end)?;
    let payload = match present {
        0 => None,
        1 => {
            let outer_size = read_u32_be_bounded(reader, block_end)?;
            let compressed_size = read_u32_le_bounded(reader, block_end)?;
            if outer_size.checked_sub(4) != Some(compressed_size) {
                return Err(block_error(
                    block_start,
                    "compressed length fields disagree",
                ));
            }
            let offset = reader.stream_position()?;
            let payload_end = offset
                .checked_add(u64::from(compressed_size))
                .ok_or(Error::OffsetOverflow)?;
            if payload_end > block_end {
                return Err(block_error(block_start, "compressed payload exceeds block"));
            }
            let prefix = if compressed_size >= 2 {
                Some(read_array_bounded(reader, payload_end)?)
            } else {
                if compressed_size == 1 {
                    read_array_bounded::<1, _>(reader, payload_end)?;
                }
                None
            };
            reader.seek(SeekFrom::Start(payload_end))?;
            Some(BlockPayload {
                offset,
                compressed_size: u64::from(compressed_size),
                prefix,
            })
        }
        _ => return Err(block_error(block_start, "invalid data-present flag")),
    };
    let end_chars = read_u32_be_bounded(reader, block_end)?;
    if end_chars != 17 {
        return Err(block_error(block_start, "invalid end-label length"));
    }
    expect_bytes(reader, block_end, BLOCK_END, block_start)?;
    if reader.stream_position()? != block_end {
        return Err(block_error(
            block_start,
            "declared block size does not match contents",
        ));
    }
    Ok(Block {
        index,
        parameters,
        payload,
        status: 0,
        checksum: 0,
    })
}

fn read_trailer_values<R: Read + Seek>(
    reader: &mut R,
    end: u64,
    block_count: usize,
    offset: u64,
) -> Result<Vec<u32>> {
    if read_u32_be_bounded(reader, end)? != 12 {
        return Err(block_error(offset, "invalid trailer header size"));
    }
    let count = read_u32_be_bounded(reader, end)?;
    if u64::from(count) != u64::try_from(block_count).unwrap_or(u64::MAX) {
        return Err(block_error(
            offset,
            "trailer count does not match block count",
        ));
    }
    if read_u32_be_bounded(reader, end)? != 4 {
        return Err(block_error(offset, "invalid trailer item size"));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(block_count)
        .map_err(|_| Error::LimitExceeded {
            resource: "block trailer allocation",
            value: block_count as u64,
            limit: block_count as u64,
        })?;
    for _ in 0..block_count {
        values.push(read_u32_be_bounded(reader, end)?);
    }
    Ok(values)
}

fn expect_bytes<R: Read + Seek>(
    reader: &mut R,
    end: u64,
    expected: &[u8],
    offset: u64,
) -> Result<()> {
    ensure_available(
        reader.stream_position()?,
        expected.len() as u64,
        end,
        offset,
    )?;
    let mut actual = vec![0; expected.len()];
    reader.read_exact(&mut actual)?;
    if actual != expected {
        return Err(block_error(offset, "unexpected UTF-16BE marker"));
    }
    Ok(())
}

fn read_u32_be_bounded<R: Read + Seek>(reader: &mut R, end: u64) -> Result<u32> {
    Ok(u32::from_be_bytes(read_array_bounded(reader, end)?))
}

fn read_u32_le_bounded<R: Read + Seek>(reader: &mut R, end: u64) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array_bounded(reader, end)?))
}

fn read_array_bounded<const N: usize, R: Read + Seek>(reader: &mut R, end: u64) -> Result<[u8; N]> {
    let offset = reader.stream_position()?;
    ensure_available(offset, N as u64, end, offset)?;
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn ensure_available(offset: u64, size: u64, end: u64, error_offset: u64) -> Result<()> {
    if offset.checked_add(size).is_none_or(|value| value > end) {
        return Err(block_error(error_offset, "unexpected end of block data"));
    }
    Ok(())
}

fn block_error(offset: u64, reason: &'static str) -> Error {
    Error::InvalidBlockData { offset, reason }
}

fn read_range<R: Read + Seek>(
    reader: &mut R,
    file_size: u64,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>> {
    ensure_file_range(file_size, offset, size)?;
    let capacity = usize::try_from(size).map_err(|_| Error::PayloadTooLarge {
        size,
        limit: usize::MAX as u64,
    })?;
    let mut data = Vec::new();
    data.try_reserve_exact(capacity)
        .map_err(|_| Error::PayloadTooLarge {
            size,
            limit: usize::MAX as u64,
        })?;
    reader.seek(SeekFrom::Start(offset))?;
    reader.take(size).read_to_end(&mut data)?;
    if data.len() as u64 != size {
        return Err(Error::ChunkOutOfBounds {
            offset,
            payload_size: size,
            file_size,
        });
    }
    Ok(data)
}

fn copy_range<R: Read + Seek, W: Write>(
    reader: &mut R,
    file_size: u64,
    offset: u64,
    size: u64,
    writer: &mut W,
) -> Result<u64> {
    ensure_file_range(file_size, offset, size)?;
    reader.seek(SeekFrom::Start(offset))?;
    let copied = std::io::copy(&mut reader.take(size), writer)?;
    if copied != size {
        return Err(Error::ChunkOutOfBounds {
            offset,
            payload_size: size,
            file_size,
        });
    }
    Ok(copied)
}

fn ensure_file_range(file_size: u64, offset: u64, size: u64) -> Result<()> {
    if offset.checked_add(size).is_none_or(|end| end > file_size) {
        return Err(Error::ChunkOutOfBounds {
            offset,
            payload_size: size,
            file_size,
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
pub(crate) struct RebuiltBlockData {
    pub(crate) body: Vec<u8>,
    pub(crate) decoded_size: u64,
    pub(crate) original_compressed_size: Option<u64>,
    pub(crate) compressed_size: u64,
    pub(crate) original_checksum: u32,
}

#[cfg(feature = "write")]
pub(crate) fn rebuild_block_data_body(
    body: &[u8],
    block_index: u32,
    decoded: &[u8],
    replacement_checksum: u32,
    max_blocks: u64,
    decoded_limit: u64,
    body_limit: u64,
) -> Result<RebuiltBlockData> {
    let body_size = body.len() as u64;
    if body_size > body_limit {
        return Err(Error::LimitExceeded {
            resource: "source block-data body size",
            value: body_size,
            limit: body_limit,
        });
    }
    let header = ExternalChunkHeader {
        identifier: Box::default(),
        body_offset: 0,
        body_size,
    };
    let mut reader = std::io::Cursor::new(body);
    let data = parse_block_data(&mut reader, &header, max_blocks)?;
    let mut matches = data
        .blocks()
        .iter()
        .filter(|block| block.index() == block_index);
    let target = matches.next().ok_or_else(|| Error::InvalidWrite {
        reason: format!("block-data object has no block index {block_index}"),
    })?;
    if matches.next().is_some() {
        return Err(Error::InvalidWrite {
            reason: format!("block-data object contains duplicate block index {block_index}"),
        });
    }

    let parameters = target.parameters();
    let expected_size = u64::from(parameters.channel_count())
        .checked_mul(u64::from(parameters.width()))
        .and_then(|value| value.checked_mul(u64::from(parameters.height())))
        .ok_or(Error::OffsetOverflow)?;
    if expected_size > decoded_limit {
        return Err(Error::LimitExceeded {
            resource: "replacement decoded block size",
            value: expected_size,
            limit: decoded_limit,
        });
    }
    if decoded.len() as u64 != expected_size {
        return Err(Error::InvalidWrite {
            reason: format!(
                "replacement for block {block_index} has {} decoded bytes, expected {expected_size}",
                decoded.len()
            ),
        });
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(decoded)?;
    let compressed = encoder.finish()?;
    let compressed_size = u64::try_from(compressed.len()).map_err(|_| Error::OffsetOverflow)?;
    let original_compressed_size = target.payload().map(|payload| payload.compressed_size());
    let removed_size = original_compressed_size
        .map(|size| size.checked_add(8).ok_or(Error::OffsetOverflow))
        .transpose()?
        .unwrap_or(0);
    let added_size = compressed_size
        .checked_add(8)
        .ok_or(Error::OffsetOverflow)?;
    let output_size = body_size
        .checked_sub(removed_size)
        .and_then(|size| size.checked_add(added_size))
        .ok_or(Error::OffsetOverflow)?;
    if output_size > body_limit {
        return Err(Error::LimitExceeded {
            resource: "replacement block-data body size",
            value: output_size,
            limit: body_limit,
        });
    }
    let output_capacity = usize::try_from(output_size).map_err(|_| Error::LimitExceeded {
        resource: "replacement block-data body size",
        value: output_size,
        limit: usize::MAX as u64,
    })?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_capacity)
        .map_err(|_| Error::LimitExceeded {
            resource: "replacement block-data body size",
            value: output_size,
            limit: body_limit,
        })?;

    for block in data.blocks() {
        let start = output.len();
        push_u32_be(&mut output, 0);
        push_u32_be(&mut output, 19);
        output.extend_from_slice(BLOCK_BEGIN);
        push_u32_be(&mut output, block.index());
        output.extend_from_slice(&block.parameters().raw());
        let is_target = block.index() == block_index;
        if is_target {
            push_payload(&mut output, &compressed)?;
        } else if let Some(payload) = block.payload() {
            let payload = body_slice(body, payload.offset(), payload.compressed_size())?;
            push_payload(&mut output, payload)?;
        } else {
            push_u32_be(&mut output, 0);
        }
        push_u32_be(&mut output, 17);
        output.extend_from_slice(BLOCK_END);
        let block_size = u32::try_from(output.len() - start).map_err(|_| Error::InvalidWrite {
            reason: "serialized block exceeds the 32-bit on-disk size".to_owned(),
        })?;
        output[start..start + 4].copy_from_slice(&block_size.to_be_bytes());
    }
    push_trailer_values(
        &mut output,
        BLOCK_STATUS,
        data.blocks().iter().map(Block::status),
    )?;
    push_trailer_values(
        &mut output,
        BLOCK_CHECKSUM,
        data.blocks().iter().map(|block| {
            if block.index() == block_index {
                replacement_checksum
            } else {
                block.checksum()
            }
        }),
    )?;
    if output.len() as u64 != output_size {
        return Err(Error::InvalidWrite {
            reason: "serialized block-data body size calculation disagrees with output".to_owned(),
        });
    }

    let validation_header = ExternalChunkHeader {
        identifier: Box::default(),
        body_offset: 0,
        body_size: output_size,
    };
    let mut validation_reader = std::io::Cursor::new(&output);
    let validated = parse_block_data(&mut validation_reader, &validation_header, max_blocks)?;
    let validated_target = validated
        .blocks()
        .iter()
        .find(|block| block.index() == block_index)
        .ok_or_else(|| Error::InvalidWrite {
            reason: "serialized block-data body lost the replacement block".to_owned(),
        })?;
    if validated_target.checksum() != replacement_checksum
        || validated_target
            .payload()
            .is_none_or(|payload| !payload.has_zlib_header())
    {
        return Err(Error::InvalidWrite {
            reason: "serialized replacement block failed validation".to_owned(),
        });
    }

    Ok(RebuiltBlockData {
        body: output,
        decoded_size: expected_size,
        original_compressed_size,
        compressed_size,
        original_checksum: target.checksum(),
    })
}

#[cfg(feature = "write")]
fn push_payload(output: &mut Vec<u8>, compressed: &[u8]) -> Result<()> {
    let size = u32::try_from(compressed.len()).map_err(|_| Error::InvalidWrite {
        reason: "compressed replacement block exceeds the 32-bit on-disk size".to_owned(),
    })?;
    push_u32_be(output, 1);
    push_u32_be(output, size.checked_add(4).ok_or(Error::OffsetOverflow)?);
    output.extend_from_slice(&size.to_le_bytes());
    output.extend_from_slice(compressed);
    Ok(())
}

#[cfg(feature = "write")]
fn push_trailer_values(
    output: &mut Vec<u8>,
    marker: &[u8],
    values: impl ExactSizeIterator<Item = u32>,
) -> Result<()> {
    let marker_chars = u32::try_from(marker.len() / 2).map_err(|_| Error::OffsetOverflow)?;
    let count = u32::try_from(values.len()).map_err(|_| Error::InvalidWrite {
        reason: "block trailer count exceeds the 32-bit on-disk size".to_owned(),
    })?;
    push_u32_be(output, marker_chars);
    output.extend_from_slice(marker);
    push_u32_be(output, 12);
    push_u32_be(output, count);
    push_u32_be(output, 4);
    for value in values {
        push_u32_be(output, value);
    }
    Ok(())
}

#[cfg(feature = "write")]
fn body_slice(body: &[u8], offset: u64, size: u64) -> Result<&[u8]> {
    let start = usize::try_from(offset).map_err(|_| Error::OffsetOverflow)?;
    let end = offset
        .checked_add(size)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(Error::OffsetOverflow)?;
    body.get(start..end).ok_or_else(|| Error::InvalidWrite {
        reason: "source block payload is outside its external body".to_owned(),
    })
}

#[cfg(feature = "write")]
fn push_u32_be(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn push_u32_be(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u32_le(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_block(bytes: &mut Vec<u8>, index: u32, payload: Option<&[u8]>) {
        let start = bytes.len();
        push_u32_be(bytes, 0);
        push_u32_be(bytes, 19);
        bytes.extend_from_slice(BLOCK_BEGIN);
        push_u32_be(bytes, index);
        bytes.extend_from_slice(&[0, 5, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0]);
        push_u32_be(bytes, u32::from(payload.is_some()));
        if let Some(payload) = payload {
            push_u32_be(bytes, payload.len() as u32 + 4);
            push_u32_le(bytes, payload.len() as u32);
            bytes.extend_from_slice(payload);
        }
        push_u32_be(bytes, 17);
        bytes.extend_from_slice(BLOCK_END);
        let size = (bytes.len() - start) as u32;
        bytes[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    fn push_trailer(bytes: &mut Vec<u8>, marker: &[u8], values: &[u32]) {
        push_u32_be(bytes, (marker.len() / 2) as u32);
        bytes.extend_from_slice(marker);
        push_u32_be(bytes, 12);
        push_u32_be(bytes, values.len() as u32);
        push_u32_be(bytes, 4);
        for value in values {
            push_u32_be(bytes, *value);
        }
    }

    #[test]
    fn parses_block_metadata_without_loading_payloads() {
        let mut bytes = Vec::new();
        push_block(&mut bytes, 0, Some(&[0x78, 0x01, 1, 2, 3]));
        push_block(&mut bytes, 1, None);
        push_trailer(&mut bytes, BLOCK_STATUS, &[1, 0]);
        push_trailer(&mut bytes, BLOCK_CHECKSUM, &[0x1234, 0]);
        let size = bytes.len() as u64;
        let mut reader = Cursor::new(bytes);
        let header = ExternalChunkHeader {
            identifier: Box::from(&b"id"[..]),
            body_offset: 0,
            body_size: size,
        };

        let data = parse_block_data(&mut reader, &header, 10).unwrap();
        assert_eq!(data.blocks().len(), 2);
        assert_eq!(data.present_blocks(), 1);
        assert_eq!(data.empty_blocks(), 1);
        assert_eq!(data.blocks()[0].index(), 0);
        assert_eq!(data.blocks()[0].parameters().channel_count(), 5);
        assert_eq!(data.blocks()[0].parameters().width(), 256);
        assert!(data.blocks()[0].payload().unwrap().has_zlib_header());
        assert_eq!(data.blocks()[0].status(), 1);
        assert_eq!(data.blocks()[0].checksum(), 0x1234);
        assert!(data.blocks()[1].payload().is_none());
        assert_eq!(data.uniform_status(), None);
    }

    #[test]
    fn reports_a_uniform_opaque_status() {
        let mut bytes = Vec::new();
        push_block(&mut bytes, 0, Some(&[0x78, 0x01]));
        push_block(&mut bytes, 1, None);
        push_trailer(&mut bytes, BLOCK_STATUS, &[0, 0]);
        push_trailer(&mut bytes, BLOCK_CHECKSUM, &[1, 0]);
        let size = bytes.len() as u64;
        let mut reader = Cursor::new(bytes);
        let header = ExternalChunkHeader {
            identifier: Box::from(&b"id"[..]),
            body_offset: 0,
            body_size: size,
        };

        let data = parse_block_data(&mut reader, &header, 10).unwrap();
        assert_eq!(data.uniform_status(), Some(0));
    }

    #[test]
    fn enforces_block_limit() {
        let mut bytes = Vec::new();
        push_block(&mut bytes, 0, None);
        push_trailer(&mut bytes, BLOCK_STATUS, &[0]);
        push_trailer(&mut bytes, BLOCK_CHECKSUM, &[0]);
        let size = bytes.len() as u64;
        let mut reader = Cursor::new(bytes);
        let header = ExternalChunkHeader {
            identifier: Box::from(&b"id"[..]),
            body_offset: 0,
            body_size: size,
        };
        assert!(matches!(
            parse_block_data(&mut reader, &header, 0),
            Err(Error::LimitExceeded {
                resource: "blocks per external object",
                ..
            })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn rebuilding_one_block_preserves_the_other_block_records() {
        let untouched_payload = [0x78, 0x01, 1, 2, 3];
        let mut bytes = Vec::new();
        push_block(&mut bytes, 0, Some(&untouched_payload));
        push_block(&mut bytes, 1, None);
        push_trailer(&mut bytes, BLOCK_STATUS, &[1, 0]);
        push_trailer(&mut bytes, BLOCK_CHECKSUM, &[0x1234, 0x5678]);

        let decoded = vec![0_u8; 5 * 256 * 256];
        let rebuilt = rebuild_block_data_body(
            &bytes,
            1,
            &decoded,
            0,
            10,
            decoded.len() as u64,
            1024 * 1024,
        )
        .unwrap();
        let header = ExternalChunkHeader {
            identifier: Box::from(&b"id"[..]),
            body_offset: 0,
            body_size: rebuilt.body.len() as u64,
        };
        let mut reader = Cursor::new(&rebuilt.body);
        let data = parse_block_data(&mut reader, &header, 10).unwrap();

        let untouched = &data.blocks()[0];
        let payload = untouched.payload().unwrap();
        assert_eq!(
            body_slice(&rebuilt.body, payload.offset(), payload.compressed_size()).unwrap(),
            untouched_payload
        );
        assert_eq!(untouched.status(), 1);
        assert_eq!(untouched.checksum(), 0x1234);
        assert_eq!(data.blocks()[1].status(), 0);
        assert_eq!(data.blocks()[1].checksum(), 0);
        assert!(data.blocks()[1].payload().unwrap().has_zlib_header());
    }

    #[test]
    fn recognizes_observed_external_body_prefixes() {
        assert!(looks_like_block_data(&[
            0, 0, 0, 104, 0, 0, 0, 19, 0, b'B', 0, b'l'
        ]));
        assert!(is_zlib_header([0x78, 0x01]));
        assert!(is_zlib_header([0x78, 0x9c]));
        assert!(!is_zlib_header([0x50, 0x4b]));
    }
}
