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

    fn database(offset: u64, raw_size: usize, stored_size: usize) -> Database {
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
                 INSERT INTO TimeLapseRecord VALUES (2, 1, 0, 'WEBP', 3, 4);
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
        let database = database(offset, raw.len(), body.len());
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
}
