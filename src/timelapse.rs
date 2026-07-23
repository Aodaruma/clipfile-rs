use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, Write},
};

use flate2::read::ZlibDecoder;
use rusqlite::types::ValueRef;

use crate::{ByteOrder, ClipFile, Database, Error, ExternalBody, Limits, Result};

/// One time-lapse blob row and its opaque external-object identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLapseBlob {
    id: i64,
    canvas_id: i64,
    next_blob_id: Option<i64>,
    offset: u64,
    decoded_size: u64,
    stored_size: u64,
    kind: i64,
    external_identifier: Box<[u8]>,
}

impl TimeLapseBlob {
    /// `TimeLapseBlob.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Next blob ID, or `None` at the end of the chain.
    #[must_use]
    pub const fn next_blob_id(&self) -> Option<i64> {
        self.next_blob_id
    }

    /// Byte offset in the reconstructed decoded stream.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Expected decoded byte count.
    #[must_use]
    pub const fn decoded_size(&self) -> u64 {
        self.decoded_size
    }

    /// Stored byte count including the four-byte compressed-length prefix.
    #[must_use]
    pub const fn stored_size(&self) -> u64 {
        self.stored_size
    }

    /// Uninterpreted `BlobType` value.
    #[must_use]
    pub const fn kind(&self) -> i64 {
        self.kind
    }

    /// Opaque external-object identifier from `BlobData`.
    #[must_use]
    pub fn external_identifier(&self) -> &[u8] {
        &self.external_identifier
    }
}

/// Raw four-byte kind at the start of one time-lapse frame record.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimeLapseFrameKind([u8; 4]);

impl TimeLapseFrameKind {
    /// Creates a frame kind without assigning unverified semantics.
    #[must_use]
    pub const fn new(raw: [u8; 4]) -> Self {
        Self(raw)
    }

    /// Original four-byte record kind.
    #[must_use]
    pub const fn raw(self) -> [u8; 4] {
        self.0
    }

    /// Whether the record kind is the observed `GMIK`.
    #[must_use]
    pub fn is_gmik(self) -> bool {
        self.0 == *b"GMIK"
    }

    /// Whether the record kind is the observed `GMID`.
    #[must_use]
    pub fn is_gmid(self) -> bool {
        self.0 == *b"GMID"
    }

    /// Whether this is a verified full-canvas key-frame record (`GMIK`).
    #[must_use]
    pub fn is_key_frame(self) -> bool {
        self.is_gmik()
    }

    /// Whether this is a verified positioned delta-patch record (`GMID`).
    #[must_use]
    pub fn is_delta_frame(self) -> bool {
        self.is_gmid()
    }
}

/// One validated frame record in a decoded time-lapse stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLapseFrame {
    kind: TimeLapseFrameKind,
    record_offset: u64,
    encoded_offset: u64,
    encoded_size: u64,
    sequence: u32,
    reserved_before_sequence: u32,
    reserved_after_sequence: u32,
    parameter_a: u32,
    parameter_b: u32,
    webp_chunk_kind: [u8; 4],
    webp_dimensions: Option<(u32, u32)>,
}

impl TimeLapseFrame {
    /// Raw `GMIK`/`GMID` record kind.
    #[must_use]
    pub const fn kind(&self) -> TimeLapseFrameKind {
        self.kind
    }

    /// Offset of the 28-byte record header in the reconstructed decoded stream.
    #[must_use]
    pub const fn record_offset(&self) -> u64 {
        self.record_offset
    }

    /// Offset of the embedded `RIFF`/`WEBP` bytes in the decoded stream.
    #[must_use]
    pub const fn encoded_offset(&self) -> u64 {
        self.encoded_offset
    }

    /// Complete embedded `RIFF` byte count.
    #[must_use]
    pub const fn encoded_size(&self) -> u64 {
        self.encoded_size
    }

    /// One-based contiguous sequence value.
    #[must_use]
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    /// First currently uninterpreted header word, observed as zero.
    #[must_use]
    pub const fn reserved_before_sequence(&self) -> u32 {
        self.reserved_before_sequence
    }

    /// Second currently uninterpreted header word, observed as zero.
    #[must_use]
    pub const fn reserved_after_sequence(&self) -> u32 {
        self.reserved_after_sequence
    }

    /// First raw region parameter.
    ///
    /// For `GMID`, this is the verified horizontal patch origin. Its meaning
    /// remains unverified for `GMIK`; use [`Self::delta_origin`] when possible.
    #[must_use]
    pub const fn parameter_a(&self) -> u32 {
        self.parameter_a
    }

    /// Second raw region parameter.
    ///
    /// For `GMID`, this is the verified vertical patch origin. Its meaning
    /// remains unverified for `GMIK`; use [`Self::delta_origin`] when possible.
    #[must_use]
    pub const fn parameter_b(&self) -> u32 {
        self.parameter_b
    }

    /// Destination origin for a verified `GMID` delta patch.
    ///
    /// Returns `None` for full-canvas `GMIK` records because the two raw
    /// parameters have a different, still-unverified meaning there.
    #[must_use]
    pub fn delta_origin(&self) -> Option<(u32, u32)> {
        self.kind
            .is_delta_frame()
            .then_some((self.parameter_a, self.parameter_b))
    }

    /// First WebP chunk kind following the `WEBP` form type.
    #[must_use]
    pub const fn webp_chunk_kind(&self) -> [u8; 4] {
        self.webp_chunk_kind
    }

    /// Dimensions decoded from an observed `VP8 ` or `VP8X` first chunk.
    #[must_use]
    pub const fn webp_dimensions(&self) -> Option<(u32, u32)> {
        self.webp_dimensions
    }
}

/// One time-lapse encoder record and its ordered blob chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLapseRecord {
    id: i64,
    canvas_id: i64,
    next_record_id: Option<i64>,
    encoder_name: String,
    encoder_sequence: i64,
    blobs: Vec<TimeLapseBlob>,
}

impl TimeLapseRecord {
    /// `TimeLapseRecord.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Next record ID, or `None` at the end of the chain.
    #[must_use]
    pub const fn next_record_id(&self) -> Option<i64> {
        self.next_record_id
    }

    /// Encoder identifier, observed as `WEBP` in the local corpus.
    #[must_use]
    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    /// Uninterpreted encoder sequence value.
    #[must_use]
    pub const fn encoder_sequence(&self) -> i64 {
        self.encoder_sequence
    }

    /// Ordered contiguous blob chain.
    #[must_use]
    pub fn blobs(&self) -> &[TimeLapseBlob] {
        &self.blobs
    }

    /// Total decoded byte count across all blobs.
    #[must_use]
    pub fn decoded_size(&self) -> u64 {
        self.blobs.iter().map(TimeLapseBlob::decoded_size).sum()
    }
}

/// One canvas-level time-lapse manager and its ordered record chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLapseManager {
    id: i64,
    canvas_id: i64,
    records: Vec<TimeLapseRecord>,
}

impl TimeLapseManager {
    /// `TimeLapseManager.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Ordered record chain.
    #[must_use]
    pub fn records(&self) -> &[TimeLapseRecord] {
        &self.records
    }
}

/// Validated time-lapse metadata for a document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLapse {
    managers: Vec<TimeLapseManager>,
}

impl TimeLapse {
    /// Canvas managers ordered by `MainId`.
    #[must_use]
    pub fn managers(&self) -> &[TimeLapseManager] {
        &self.managers
    }
}

#[derive(Clone)]
struct RecordRow {
    id: i64,
    canvas_id: i64,
    next_record_id: Option<i64>,
    encoder_name: String,
    encoder_sequence: i64,
    first_blob_id: Option<i64>,
}

impl Database {
    /// Reads and validates the document's time-lapse record and blob chains.
    ///
    /// Documents without time-lapse tables return `None`.
    pub fn time_lapse(&self, limits: Limits) -> Result<Option<TimeLapse>> {
        let present = ["TimeLapseManager", "TimeLapseRecord", "TimeLapseBlob"]
            .map(|table| self.schema().table(table).is_some());
        if present.iter().all(|value| !value) {
            return Ok(None);
        }
        if present.iter().any(|value| !value) {
            return Err(time_lapse_error(
                "time-lapse tables are only partially present",
            ));
        }
        require_columns(
            self,
            "TimeLapseManager",
            &["MainId", "CanvasId", "RecordFirstIndex"],
        )?;
        require_columns(
            self,
            "TimeLapseRecord",
            &[
                "MainId",
                "CanvasId",
                "NextIndex",
                "EncoderName",
                "EncoderSequence",
                "BlobFirstIndex",
            ],
        )?;
        require_columns(
            self,
            "TimeLapseBlob",
            &[
                "MainId",
                "CanvasId",
                "NextIndex",
                "BlobOffset",
                "BlobSize",
                "BlobSizeCompressed",
                "BlobType",
                "BlobData",
            ],
        )?;

        let blobs = read_blob_rows(self, limits)?;
        let records = read_record_rows(self, limits)?;
        let managers = read_managers(self, &records, &blobs, limits)?;
        Ok(Some(TimeLapse { managers }))
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Reads and decodes one time-lapse blob after enforcing its size limit.
    pub fn read_time_lapse_blob(
        &mut self,
        database: &Database,
        blob: &TimeLapseBlob,
        limits: Limits,
    ) -> Result<Vec<u8>> {
        enforce_blob_size(blob.decoded_size, limits)?;
        let capacity = usize::try_from(blob.decoded_size).map_err(|_| Error::LimitExceeded {
            resource: "decoded time-lapse blob allocation",
            value: blob.decoded_size,
            limit: limits.max_time_lapse_blob_bytes(),
        })?;
        let mut decoded = Vec::new();
        decoded
            .try_reserve_exact(capacity)
            .map_err(|_| Error::LimitExceeded {
                resource: "decoded time-lapse blob allocation",
                value: blob.decoded_size,
                limit: limits.max_time_lapse_blob_bytes(),
            })?;
        self.copy_time_lapse_blob(database, blob, limits, &mut decoded)?;
        Ok(decoded)
    }

    /// Decodes one time-lapse blob into a writer and returns the byte count.
    pub fn copy_time_lapse_blob<W: Write>(
        &mut self,
        database: &Database,
        blob: &TimeLapseBlob,
        limits: Limits,
        writer: &mut W,
    ) -> Result<u64> {
        enforce_blob_size(blob.decoded_size, limits)?;
        if blob.stored_size < 4
            || blob.stored_size > limits.max_time_lapse_blob_bytes().saturating_add(4)
        {
            return Err(Error::LimitExceeded {
                resource: "stored time-lapse blob bytes",
                value: blob.stored_size,
                limit: limits.max_time_lapse_blob_bytes().saturating_add(4),
            });
        }
        let object = self
            .resolve_external_object(database, blob.external_identifier())?
            .ok_or_else(|| {
                time_lapse_error(format!("blob {} external data is missing", blob.id))
            })?;
        let ExternalBody::LengthPrefixedZlib(stream) = object.body() else {
            return Err(time_lapse_error(format!(
                "blob {} is not a length-prefixed zlib stream",
                blob.id
            )));
        };
        if stream.byte_order() != ByteOrder::BigEndian {
            return Err(time_lapse_error(format!(
                "blob {} compressed length is not big-endian",
                blob.id
            )));
        }
        if stream.compressed_size().checked_add(4) != Some(blob.stored_size) {
            return Err(time_lapse_error(format!(
                "blob {} stored size does not match its external object",
                blob.id
            )));
        }
        let compressed =
            self.read_length_prefixed_zlib(stream, limits.max_time_lapse_blob_bytes())?;
        let decoder = ZlibDecoder::new(compressed.as_slice());
        let copied = std::io::copy(
            &mut decoder.take(blob.decoded_size.saturating_add(1)),
            writer,
        )?;
        if copied != blob.decoded_size {
            return Err(time_lapse_error(format!(
                "blob {} decoded to {copied} bytes instead of {}",
                blob.id, blob.decoded_size
            )));
        }
        Ok(copied)
    }

    /// Streams and validates a record's internal `GMIK`/`GMID` frame index.
    ///
    /// Encoded WebP payloads are skipped rather than retained. The returned
    /// metadata allocation is bounded by [`Limits::max_time_lapse_items`].
    pub fn read_time_lapse_frame_index(
        &mut self,
        database: &Database,
        record: &TimeLapseRecord,
        limits: Limits,
    ) -> Result<Vec<TimeLapseFrame>> {
        if record.encoder_name != "WEBP" {
            return Err(time_lapse_error(format!(
                "record {} uses unsupported frame encoder {:?}",
                record.id, record.encoder_name
            )));
        }
        let mut scanner = TimeLapseFrameScanner::new(limits);
        for blob in &record.blobs {
            if let Err(error) = self.copy_time_lapse_blob(database, blob, limits, &mut scanner) {
                if let Some(failure) = scanner.take_failure() {
                    return Err(failure);
                }
                return Err(error);
            }
        }
        scanner.finish(record)
    }
}

struct TimeLapseFrameScanner {
    limits: Limits,
    offset: u64,
    previous_end: u64,
    phase: TimeLapseFrameScanPhase,
    search_window: Vec<u8>,
    record_header: Vec<u8>,
    frames: Vec<TimeLapseFrame>,
    failure: Option<Error>,
}

enum TimeLapseFrameScanPhase {
    Header,
    RiffHeader { start: u64, bytes: Vec<u8> },
    RiffBody { end: u64 },
}

impl TimeLapseFrameScanner {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            offset: 0,
            previous_end: 0,
            phase: TimeLapseFrameScanPhase::Header,
            search_window: Vec::new(),
            record_header: Vec::new(),
            frames: Vec::new(),
            failure: None,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
        let mut input = bytes;
        while !input.is_empty() {
            let mut completed_riff = None;
            match &mut self.phase {
                TimeLapseFrameScanPhase::RiffBody { end } => {
                    let remaining = end
                        .checked_sub(self.offset)
                        .ok_or("time-lapse stream crossed a RIFF boundary")?;
                    let consumed = remaining.min(input.len() as u64) as usize;
                    self.offset = self
                        .offset
                        .checked_add(consumed as u64)
                        .ok_or("time-lapse stream offset overflow")?;
                    input = &input[consumed..];
                    if self.offset == *end {
                        self.phase = TimeLapseFrameScanPhase::Header;
                    }
                }
                TimeLapseFrameScanPhase::Header => {
                    let byte = input[0];
                    input = &input[1..];
                    self.record_header.push(byte);
                    self.search_window.push(byte);
                    if self.search_window.len() > 4 {
                        self.search_window.remove(0);
                    }
                    self.offset = self
                        .offset
                        .checked_add(1)
                        .ok_or("time-lapse stream offset overflow")?;
                    if self.search_window == b"RIFF" {
                        let start = self.offset - 4;
                        self.record_header
                            .truncate(self.record_header.len().saturating_sub(4));
                        if self.record_header.len() != 28 {
                            return Err(format!(
                                "time-lapse frame header is {} bytes instead of 28",
                                self.record_header.len()
                            ));
                        }
                        if start.checked_sub(self.previous_end) != Some(28) {
                            return Err(
                                "time-lapse frames are not contiguous 28-byte records".to_owned()
                            );
                        }
                        self.search_window.clear();
                        self.phase = TimeLapseFrameScanPhase::RiffHeader {
                            start,
                            bytes: b"RIFF".to_vec(),
                        };
                    } else if self.record_header.len() > 31 {
                        return Err("time-lapse frame header has no RIFF marker".to_owned());
                    }
                }
                TimeLapseFrameScanPhase::RiffHeader {
                    start,
                    bytes: header,
                } => {
                    let wanted = 30 - header.len();
                    let consumed = wanted.min(input.len());
                    header.extend_from_slice(&input[..consumed]);
                    input = &input[consumed..];
                    self.offset = self
                        .offset
                        .checked_add(consumed as u64)
                        .ok_or("time-lapse stream offset overflow")?;
                    if header.len() == 30 {
                        completed_riff = Some((*start, header.clone()));
                    }
                }
            }
            if let Some((start, header)) = completed_riff {
                let end = self.finish_riff_header(start, &header)?;
                self.previous_end = end;
                self.record_header.clear();
                self.phase = if end == self.offset {
                    TimeLapseFrameScanPhase::Header
                } else {
                    TimeLapseFrameScanPhase::RiffBody { end }
                };
            }
        }
        Ok(())
    }

    fn finish_riff_header(
        &mut self,
        riff_start: u64,
        riff: &[u8],
    ) -> std::result::Result<u64, String> {
        if &riff[8..12] != b"WEBP" {
            return Err("time-lapse RIFF record does not contain WEBP".to_owned());
        }
        let encoded_size = u64::from(u32::from_le_bytes(riff[4..8].try_into().unwrap()))
            .checked_add(8)
            .ok_or("time-lapse RIFF size overflow")?;
        if encoded_size < 30 {
            return Err(format!("time-lapse WebP is only {encoded_size} bytes"));
        }
        let end = riff_start
            .checked_add(encoded_size)
            .ok_or("time-lapse RIFF end overflow")?;
        if end < self.offset {
            return Err("time-lapse RIFF size is shorter than its header".to_owned());
        }

        let words = self
            .record_header
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        let words: [u32; 7] = words.try_into().expect("validated 28-byte header");
        if u64::from(words[1]) != encoded_size.saturating_add(16) {
            return Err("time-lapse record size does not match its WebP payload".to_owned());
        }
        let expected_sequence = u32::try_from(self.frames.len() + 1)
            .map_err(|_| "time-lapse frame sequence exceeds u32")?;
        if words[3] != expected_sequence {
            return Err(format!(
                "time-lapse frame sequence is {} instead of {expected_sequence}",
                words[3]
            ));
        }
        let frame_count = self.frames.len() as u64 + 1;
        if frame_count > self.limits.max_time_lapse_items() {
            self.failure = Some(Error::LimitExceeded {
                resource: "time-lapse frames",
                value: frame_count,
                limit: self.limits.max_time_lapse_items(),
            });
            return Err("time-lapse frame count exceeds its limit".to_owned());
        }
        let kind = TimeLapseFrameKind::new(self.record_header[0..4].try_into().unwrap());
        let webp_chunk_kind = riff[12..16].try_into().unwrap();
        let webp_dimensions = parse_webp_dimensions(riff, webp_chunk_kind, encoded_size)?;
        for value in webp_dimensions
            .into_iter()
            .flat_map(|(width, height)| [width, height])
        {
            if value == 0 || value > self.limits.max_canvas_dimension() {
                return Err(format!(
                    "time-lapse WebP dimension {value} exceeds its limit"
                ));
            }
        }
        self.frames.push(TimeLapseFrame {
            kind,
            record_offset: riff_start - 28,
            encoded_offset: riff_start,
            encoded_size,
            sequence: words[3],
            reserved_before_sequence: words[2],
            reserved_after_sequence: words[4],
            parameter_a: words[5],
            parameter_b: words[6],
            webp_chunk_kind,
            webp_dimensions,
        });
        Ok(end)
    }

    fn finish(self, record: &TimeLapseRecord) -> Result<Vec<TimeLapseFrame>> {
        if !matches!(self.phase, TimeLapseFrameScanPhase::Header)
            || !self.record_header.is_empty()
            || self.offset != record.decoded_size()
        {
            return Err(time_lapse_error(format!(
                "record {} ends inside a time-lapse frame",
                record.id
            )));
        }
        let expected = u64::try_from(record.encoder_sequence).map_err(|_| {
            time_lapse_error(format!(
                "record {} has a negative encoder sequence",
                record.id
            ))
        })?;
        if self.frames.len() as u64 != expected {
            return Err(time_lapse_error(format!(
                "record {} contains {} frames instead of its encoder sequence {expected}",
                record.id,
                self.frames.len()
            )));
        }
        Ok(self.frames)
    }

    fn take_failure(&mut self) -> Option<Error> {
        self.failure.take()
    }
}

impl Write for TimeLapseFrameScanner {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(reason) = self.feed(bytes) {
            if self.failure.is_none() {
                self.failure = Some(time_lapse_error(reason.clone()));
            }
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, reason));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn parse_webp_dimensions(
    header: &[u8],
    chunk_kind: [u8; 4],
    encoded_size: u64,
) -> std::result::Result<Option<(u32, u32)>, String> {
    let chunk_size = u64::from(u32::from_le_bytes(header[16..20].try_into().unwrap()));
    let padded_chunk_size = chunk_size
        .checked_add(chunk_size & 1)
        .ok_or("time-lapse WebP chunk size overflow")?;
    let first_chunk_end = 20_u64
        .checked_add(padded_chunk_size)
        .ok_or("time-lapse WebP chunk end overflow")?;
    if first_chunk_end > encoded_size {
        return Err("time-lapse WebP first chunk exceeds its RIFF size".to_owned());
    }
    match &chunk_kind {
        b"VP8X" => {
            if chunk_size < 10 {
                return Err("time-lapse VP8X chunk is shorter than its header".to_owned());
            }
            Ok(Some((
                1 + little_endian_u24(&header[24..27]),
                1 + little_endian_u24(&header[27..30]),
            )))
        }
        b"VP8 " => {
            if chunk_size < 10 {
                return Err("time-lapse VP8 chunk is shorter than its frame header".to_owned());
            }
            if &header[23..26] != b"\x9d\x01\x2a" {
                return Err("time-lapse VP8 frame lacks its start code".to_owned());
            }
            Ok(Some((
                u32::from(u16::from_le_bytes([header[26], header[27]]) & 0x3fff),
                u32::from(u16::from_le_bytes([header[28], header[29]]) & 0x3fff),
            )))
        }
        _ => Ok(None),
    }
}

fn little_endian_u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn read_blob_rows(database: &Database, limits: Limits) -> Result<BTreeMap<i64, TimeLapseBlob>> {
    let mut statement = database.connection().prepare(
        "SELECT MainId, CanvasId, NextIndex, BlobOffset, BlobSize, BlobSizeCompressed, \
                BlobType, BlobData FROM TimeLapseBlob ORDER BY MainId",
    )?;
    let mut rows = statement.query([])?;
    let mut blobs = BTreeMap::new();
    while let Some(row) = rows.next()? {
        enforce_items(blobs.len() as u64 + 1, limits, "time-lapse blobs")?;
        let id: i64 = row.get(0)?;
        let external = required_bytes(row.get_ref(7)?, 7, "BlobData")?;
        if external.len() as u64 > limits.max_identifier_size() {
            return Err(Error::LimitExceeded {
                resource: "time-lapse external identifier",
                value: external.len() as u64,
                limit: limits.max_identifier_size(),
            });
        }
        let blob = TimeLapseBlob {
            id,
            canvas_id: row.get(1)?,
            next_blob_id: nonzero(row.get(2)?),
            offset: nonnegative(row.get(3)?, "BlobOffset")?,
            decoded_size: nonnegative(row.get(4)?, "BlobSize")?,
            stored_size: nonnegative(row.get(5)?, "BlobSizeCompressed")?,
            kind: row.get(6)?,
            external_identifier: Box::from(external),
        };
        enforce_blob_size(blob.decoded_size, limits)?;
        if blob.stored_size < 4 {
            return Err(time_lapse_error(format!(
                "blob {id} stored size is shorter than its length prefix"
            )));
        }
        if blob.stored_size > limits.max_time_lapse_blob_bytes().saturating_add(4) {
            return Err(Error::LimitExceeded {
                resource: "stored time-lapse blob bytes",
                value: blob.stored_size,
                limit: limits.max_time_lapse_blob_bytes().saturating_add(4),
            });
        }
        if blobs.insert(id, blob).is_some() {
            return Err(time_lapse_error(format!("duplicate blob ID {id}")));
        }
    }
    Ok(blobs)
}

fn read_record_rows(database: &Database, limits: Limits) -> Result<BTreeMap<i64, RecordRow>> {
    let mut statement = database.connection().prepare(
        "SELECT MainId, CanvasId, NextIndex, EncoderName, EncoderSequence, BlobFirstIndex \
         FROM TimeLapseRecord ORDER BY MainId",
    )?;
    let mut rows = statement.query([])?;
    let mut records = BTreeMap::new();
    let mut encoder_bytes = 0_u64;
    while let Some(row) = rows.next()? {
        enforce_items(records.len() as u64 + 1, limits, "time-lapse records")?;
        let id: i64 = row.get(0)?;
        let encoder = required_text(row.get_ref(3)?, 3, "EncoderName")?;
        encoder_bytes = encoder_bytes
            .checked_add(encoder.len() as u64)
            .ok_or(Error::OffsetOverflow)?;
        if encoder_bytes > limits.max_time_lapse_blob_bytes() {
            return Err(Error::LimitExceeded {
                resource: "time-lapse encoder names",
                value: encoder_bytes,
                limit: limits.max_time_lapse_blob_bytes(),
            });
        }
        let record = RecordRow {
            id,
            canvas_id: row.get(1)?,
            next_record_id: nonzero(row.get(2)?),
            encoder_name: encoder.to_owned(),
            encoder_sequence: row.get(4)?,
            first_blob_id: nonzero(row.get(5)?),
        };
        if records.insert(id, record).is_some() {
            return Err(time_lapse_error(format!("duplicate record ID {id}")));
        }
    }
    Ok(records)
}

fn read_managers(
    database: &Database,
    records: &BTreeMap<i64, RecordRow>,
    blobs: &BTreeMap<i64, TimeLapseBlob>,
    limits: Limits,
) -> Result<Vec<TimeLapseManager>> {
    let mut statement = database.connection().prepare(
        "SELECT MainId, CanvasId, RecordFirstIndex FROM TimeLapseManager ORDER BY MainId",
    )?;
    let managers = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                nonzero(row.get(2)?),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    enforce_items(managers.len() as u64, limits, "time-lapse managers")?;
    let mut used_records = BTreeSet::new();
    let mut used_blobs = BTreeSet::new();
    let mut result = Vec::new();
    for (id, canvas_id, first_record_id) in managers {
        let mut current = first_record_id;
        let mut ordered_records = Vec::new();
        while let Some(record_id) = current {
            if !used_records.insert(record_id) {
                return Err(time_lapse_error(format!(
                    "record {record_id} is cyclic or shared"
                )));
            }
            let row = records
                .get(&record_id)
                .ok_or_else(|| time_lapse_error(format!("record {record_id} is missing")))?;
            if row.canvas_id != canvas_id {
                return Err(time_lapse_error(format!(
                    "record {record_id} belongs to a different canvas"
                )));
            }
            let ordered_blobs = order_blobs(row, blobs, &mut used_blobs, limits)?;
            ordered_records.push(TimeLapseRecord {
                id: row.id,
                canvas_id: row.canvas_id,
                next_record_id: row.next_record_id,
                encoder_name: row.encoder_name.clone(),
                encoder_sequence: row.encoder_sequence,
                blobs: ordered_blobs,
            });
            current = row.next_record_id;
        }
        result.push(TimeLapseManager {
            id,
            canvas_id,
            records: ordered_records,
        });
    }
    if used_records.len() != records.len() || used_blobs.len() != blobs.len() {
        return Err(time_lapse_error(
            "time-lapse tables contain unreachable records or blobs",
        ));
    }
    Ok(result)
}

fn order_blobs(
    record: &RecordRow,
    blobs: &BTreeMap<i64, TimeLapseBlob>,
    used: &mut BTreeSet<i64>,
    limits: Limits,
) -> Result<Vec<TimeLapseBlob>> {
    let mut current = record.first_blob_id;
    let mut offset = 0_u64;
    let mut ordered = Vec::new();
    while let Some(blob_id) = current {
        enforce_items(ordered.len() as u64 + 1, limits, "time-lapse blob chain")?;
        if !used.insert(blob_id) {
            return Err(time_lapse_error(format!(
                "blob {blob_id} is cyclic or shared"
            )));
        }
        let blob = blobs
            .get(&blob_id)
            .ok_or_else(|| time_lapse_error(format!("blob {blob_id} is missing")))?;
        if blob.canvas_id != record.canvas_id || blob.offset != offset {
            return Err(time_lapse_error(format!(
                "blob {blob_id} has a noncontiguous offset or different canvas"
            )));
        }
        offset = offset
            .checked_add(blob.decoded_size)
            .ok_or(Error::OffsetOverflow)?;
        ordered.push(blob.clone());
        current = blob.next_blob_id;
    }
    Ok(ordered)
}

fn require_columns(
    database: &Database,
    table: &'static str,
    columns: &[&'static str],
) -> Result<()> {
    for column in columns {
        database.require_column(table, column)?;
    }
    Ok(())
}

fn enforce_items(value: u64, limits: Limits, resource: &'static str) -> Result<()> {
    if value > limits.max_time_lapse_items() {
        return Err(Error::LimitExceeded {
            resource,
            value,
            limit: limits.max_time_lapse_items(),
        });
    }
    Ok(())
}

fn enforce_blob_size(value: u64, limits: Limits) -> Result<()> {
    if value > limits.max_time_lapse_blob_bytes() {
        return Err(Error::LimitExceeded {
            resource: "decoded time-lapse blob bytes",
            value,
            limit: limits.max_time_lapse_blob_bytes(),
        });
    }
    Ok(())
}

fn nonnegative(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| time_lapse_error(format!("{field} is negative")))
}

fn nonzero(value: i64) -> Option<i64> {
    (value != 0).then_some(value)
}

fn required_bytes<'a>(
    value: ValueRef<'a>,
    column: usize,
    name: &str,
) -> rusqlite::Result<&'a [u8]> {
    match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(bytes),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn required_text<'a>(value: ValueRef<'a>, column: usize, name: &str) -> rusqlite::Result<&'a str> {
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            std::str::from_utf8(bytes).map_err(|error| rusqlite::Error::Utf8Error(column, error))
        }
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn time_lapse_error(reason: impl Into<String>) -> Error {
    Error::InvalidTimeLapse {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::ZlibEncoder};
    use rusqlite::{Connection, params};

    use super::*;

    const IDENTIFIER: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";

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

    fn sample(body: &[u8]) -> (Vec<u8>, u64) {
        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);
        let mut header = Vec::new();
        push_u64(&mut header, 256);
        let database_offset_position = header.len();
        push_u64(&mut header, 0);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);
        push_chunk(&mut bytes, b"CHNKHead", &header);

        let mut external = Vec::new();
        push_u64(&mut external, IDENTIFIER.len() as u64);
        external.extend_from_slice(IDENTIFIER);
        push_u64(&mut external, body.len() as u64);
        external.extend_from_slice(body);
        let external_offset = push_chunk(&mut bytes, b"CHNKExta", &external);
        let database_offset = push_chunk(&mut bytes, b"CHNKSQLi", b"db!");
        push_chunk(&mut bytes, b"CHNKFoot", b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        let database_field = 24 + 16 + database_offset_position;
        bytes[database_field..database_field + 8].copy_from_slice(&database_offset.to_be_bytes());
        (bytes, external_offset)
    }

    fn database(
        offset: u64,
        raw_size: usize,
        stored_size: usize,
        encoder_sequence: i64,
    ) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE TimeLapseManager (
                    MainId INTEGER, CanvasId INTEGER, RecordFirstIndex INTEGER
                 );
                 INSERT INTO TimeLapseManager VALUES (1, 1, 2);
                 CREATE TABLE TimeLapseRecord (
                    MainId INTEGER, CanvasId INTEGER, NextIndex INTEGER,
                    EncoderName TEXT, EncoderSequence INTEGER, BlobFirstIndex INTEGER
                 );
                 CREATE TABLE TimeLapseBlob (
                    MainId INTEGER, CanvasId INTEGER, NextIndex INTEGER,
                    BlobOffset INTEGER, BlobSize INTEGER, BlobSizeCompressed INTEGER,
                    BlobType INTEGER, BlobData BLOB
                 );
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO TimeLapseRecord VALUES (2, 1, 0, 'WEBP', ?1, 4)",
                [encoder_sequence],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO TimeLapseBlob VALUES (4, 1, 0, 0, ?1, ?2, 2, ?3)",
                params![raw_size as i64, stored_size as i64, IDENTIFIER],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2)",
                params![IDENTIFIER, offset as i64],
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    fn push_le_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn vp8x(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::from(b"RIFF".as_slice());
        push_le_u32(&mut bytes, 22);
        bytes.extend_from_slice(b"WEBPVP8X");
        push_le_u32(&mut bytes, 10);
        bytes.extend_from_slice(&[0; 4]);
        for value in [width - 1, height - 1] {
            bytes.push(value as u8);
            bytes.push((value >> 8) as u8);
            bytes.push((value >> 16) as u8);
        }
        assert_eq!(bytes.len(), 30);
        bytes
    }

    fn frame(kind: &[u8; 4], sequence: u32, parameter_a: u32, parameter_b: u32) -> Vec<u8> {
        let webp = vp8x(64, 32);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(kind);
        push_le_u32(&mut bytes, webp.len() as u32 + 16);
        push_le_u32(&mut bytes, 0);
        push_le_u32(&mut bytes, sequence);
        push_le_u32(&mut bytes, 0);
        push_le_u32(&mut bytes, parameter_a);
        push_le_u32(&mut bytes, parameter_b);
        bytes.extend_from_slice(&webp);
        bytes
    }

    fn encoded_body(raw: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        body.extend_from_slice(&compressed);
        body
    }

    #[test]
    fn reads_metadata_and_streams_a_blob() {
        let raw = b"GMIK\0\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0RIFF";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        body.extend_from_slice(&compressed);
        let (bytes, offset) = sample(&body);
        let database = database(offset, raw.len(), body.len(), 3);
        let time_lapse = database.time_lapse(Limits::default()).unwrap().unwrap();
        let manager = &time_lapse.managers()[0];
        let record = &manager.records()[0];
        let blob = &record.blobs()[0];
        assert_eq!(record.encoder_name(), "WEBP");
        assert_eq!(record.decoded_size(), raw.len() as u64);

        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert_eq!(
            clip.read_time_lapse_blob(&database, blob, Limits::default())
                .unwrap(),
            raw
        );
        let mut copied = Vec::new();
        assert_eq!(
            clip.copy_time_lapse_blob(&database, blob, Limits::default(), &mut copied)
                .unwrap(),
            raw.len() as u64
        );
        assert_eq!(copied, raw);
    }

    #[test]
    fn rejects_limits_and_broken_chains() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE TimeLapseManager (
                    MainId INTEGER, CanvasId INTEGER, RecordFirstIndex INTEGER
                 );
                 INSERT INTO TimeLapseManager VALUES (1, 1, 2);
                 CREATE TABLE TimeLapseRecord (
                    MainId INTEGER, CanvasId INTEGER, NextIndex INTEGER,
                    EncoderName TEXT, EncoderSequence INTEGER, BlobFirstIndex INTEGER
                 );
                 INSERT INTO TimeLapseRecord VALUES (2, 1, 2, 'WEBP', 3, 0);
                 CREATE TABLE TimeLapseBlob (
                    MainId INTEGER, CanvasId INTEGER, NextIndex INTEGER,
                    BlobOffset INTEGER, BlobSize INTEGER, BlobSizeCompressed INTEGER,
                    BlobType INTEGER, BlobData BLOB
                 );",
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        assert!(matches!(
            database.time_lapse(Limits::default()),
            Err(Error::InvalidTimeLapse { .. })
        ));
        assert!(matches!(
            database.time_lapse(Limits::default().with_max_time_lapse_items(0)),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn indexes_internal_webp_frames_across_the_decoded_stream() {
        let mut raw = frame(b"GMIK", 1, 0, 0);
        raw.extend_from_slice(&frame(b"GMID", 2, 7, 9));
        let body = encoded_body(&raw);
        let (bytes, offset) = sample(&body);
        let database = database(offset, raw.len(), body.len(), 2);
        let time_lapse = database.time_lapse(Limits::default()).unwrap().unwrap();
        let record = &time_lapse.managers()[0].records()[0];
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        let frames = clip
            .read_time_lapse_frame_index(&database, record, Limits::default())
            .unwrap();

        assert_eq!(frames.len(), 2);
        assert!(frames[0].kind().is_gmik());
        assert!(frames[1].kind().is_gmid());
        assert!(frames[0].kind().is_key_frame());
        assert!(frames[1].kind().is_delta_frame());
        assert_eq!(frames[0].record_offset(), 0);
        assert_eq!(frames[0].encoded_offset(), 28);
        assert_eq!(frames[0].encoded_size(), 30);
        assert_eq!(frames[1].record_offset(), 58);
        assert_eq!(frames[1].sequence(), 2);
        assert_eq!(frames[1].parameter_a(), 7);
        assert_eq!(frames[1].parameter_b(), 9);
        assert_eq!(frames[0].delta_origin(), None);
        assert_eq!(frames[1].delta_origin(), Some((7, 9)));
        assert_eq!(frames[1].webp_chunk_kind(), *b"VP8X");
        assert_eq!(frames[1].webp_dimensions(), Some((64, 32)));
    }

    #[test]
    fn rejects_malformed_or_oversized_frame_indexes() {
        let mut raw = frame(b"GMIK", 1, 0, 0);
        raw.extend_from_slice(&frame(b"GMID", 2, 0, 0));
        let body = encoded_body(&raw);
        let (bytes, offset) = sample(&body);
        let valid_database = database(offset, raw.len(), body.len(), 2);
        let time_lapse = valid_database
            .time_lapse(Limits::default())
            .unwrap()
            .unwrap();
        let record = &time_lapse.managers()[0].records()[0];
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            clip.read_time_lapse_frame_index(
                &valid_database,
                record,
                Limits::default().with_max_time_lapse_items(1)
            ),
            Err(Error::LimitExceeded { .. })
        ));

        raw[4..8].copy_from_slice(&0_u32.to_le_bytes());
        let body = encoded_body(&raw);
        let (bytes, offset) = sample(&body);
        let database = database(offset, raw.len(), body.len(), 2);
        let time_lapse = database.time_lapse(Limits::default()).unwrap().unwrap();
        let record = &time_lapse.managers()[0].records()[0];
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            clip.read_time_lapse_frame_index(&database, record, Limits::default()),
            Err(Error::InvalidTimeLapse { .. })
        ));
    }
}
