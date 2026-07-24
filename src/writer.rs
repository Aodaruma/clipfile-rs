use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

use rusqlite::{Connection, MAIN_DB, params};

use crate::{
    CHUNK_HEADER_SIZE, ChunkHeader, ChunkKind, ClipFile, Database, DatabaseSchema, Error,
    ExternalChunkHeader, ROOT_HEADER_SIZE, Result,
};

const ROOT_MAGIC: &[u8; 8] = b"CSFCHUNK";
const FILE_HEADER_TAG: &[u8; 8] = b"CHNKHead";
const EXTERNAL_TAG: &[u8; 8] = b"CHNKExta";
const SQLITE_TAG: &[u8; 8] = b"CHNKSQLi";
const FOOTER_TAG: &[u8; 8] = b"CHNKFoot";

/// An editable, in-memory copy of a CLIP file's embedded SQLite database.
///
/// Use [`Self::connection`] for explicit SQL changes. A [`ClipWriter`] writes
/// from a private clone of this database, repairs every `ExternalChunk.Offset`,
/// and leaves this value unchanged.
pub struct EditableDatabase {
    database: Database,
}

impl EditableDatabase {
    /// Runtime schema discovered before edits were made.
    #[must_use]
    pub const fn schema(&self) -> &DatabaseSchema {
        self.database.schema()
    }

    /// Writable SQLite connection.
    ///
    /// Schema changes are possible through this low-level API, but removing or
    /// corrupting the `ExternalChunk` index causes the writer to reject output.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        self.database.connection()
    }

    /// Mutably borrows the writable SQLite connection.
    ///
    /// This is useful for a checked [`rusqlite::Transaction`]. The transaction
    /// must be committed or rolled back before writing the container.
    #[must_use]
    pub fn connection_mut(&mut self) -> &mut Connection {
        self.database.connection_mut()
    }

    /// Runs SQLite's bounded quick integrity check.
    pub fn quick_check(&self) -> Result<()> {
        self.database.quick_check()
    }
}

/// Result of one validated CLIP container rewrite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteSummary {
    original_file_size: u64,
    output_file_size: u64,
    database_offset: u64,
    database_payload_size: u64,
    external_chunks: u64,
    replaced_external_bodies: u64,
}

impl WriteSummary {
    /// Original source size.
    #[must_use]
    pub const fn original_file_size(self) -> u64 {
        self.original_file_size
    }

    /// Rewritten output size.
    #[must_use]
    pub const fn output_file_size(self) -> u64 {
        self.output_file_size
    }

    /// Recalculated absolute offset of the `CHNKSQLi` header.
    #[must_use]
    pub const fn database_offset(self) -> u64 {
        self.database_offset
    }

    /// Serialized SQLite payload size.
    #[must_use]
    pub const fn database_payload_size(self) -> u64 {
        self.database_payload_size
    }

    /// Number of preserved external chunks.
    #[must_use]
    pub const fn external_chunks(self) -> u64 {
        self.external_chunks
    }

    /// Number of external bodies replaced by the caller.
    #[must_use]
    pub const fn replaced_external_bodies(self) -> u64 {
        self.replaced_external_bodies
    }
}

/// A validated rewrite session borrowing one source CLIP file.
///
/// The writer currently preserves the observed top-level layout, unknown
/// SQLite columns, and every unchanged external body. Replacements must supply
/// one complete external body; this API does not claim to synthesize internal
/// block checksums.
pub struct ClipWriter<'source, R> {
    source: &'source mut ClipFile<R>,
    database: EditableDatabase,
    external_replacements: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl<R: Read + Seek> ClipFile<R> {
    /// Starts an editable rewrite session after strict container validation.
    ///
    /// The source is borrowed for the lifetime of the returned writer and is
    /// never modified.
    pub fn writer(&mut self) -> Result<ClipWriter<'_, R>> {
        self.validate()?;
        let database = open_editable_database(self)?;
        Ok(ClipWriter {
            source: self,
            database: EditableDatabase { database },
            external_replacements: BTreeMap::new(),
        })
    }
}

impl<R: Read + Seek> ClipWriter<'_, R> {
    /// Editable in-memory database used for the rewrite.
    #[must_use]
    pub const fn database(&self) -> &EditableDatabase {
        &self.database
    }

    /// Mutably borrows the editable in-memory database.
    #[must_use]
    pub fn database_mut(&mut self) -> &mut EditableDatabase {
        &mut self.database
    }

    /// Replaces one complete `CHNKExta` body while preserving its identifier.
    ///
    /// The identifier must already exist in the source. The writer updates all
    /// SQLite external offsets after accounting for changed body lengths.
    pub fn replace_external_body(
        &mut self,
        identifier: impl AsRef<[u8]>,
        body: impl Into<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        let identifier = identifier.as_ref();
        if identifier.len() as u64 > self.source.limits().max_identifier_size() {
            return Err(Error::InvalidWrite {
                reason: "replacement external identifier exceeds the safety limit".to_owned(),
            });
        }
        let body = body.into();
        let limit = self.source.limits().max_write_external_body_size();
        if body.len() as u64 > limit {
            return Err(Error::LimitExceeded {
                resource: "replacement external body size",
                value: body.len() as u64,
                limit,
            });
        }
        Ok(self.external_replacements.insert(identifier.to_vec(), body))
    }

    /// Removes a pending external-body replacement.
    pub fn remove_external_replacement(&mut self, identifier: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        self.external_replacements.remove(identifier.as_ref())
    }

    /// Number of pending external-body replacements.
    #[must_use]
    pub fn replacement_count(&self) -> usize {
        self.external_replacements.len()
    }

    /// Rebuilds the container into a caller-provided stream.
    ///
    /// This method does not flush or validate the destination after the final
    /// write. Prefer [`Self::write_to_path`] when writing a file.
    pub fn write_to<W: Write>(&mut self, destination: &mut W) -> Result<WriteSummary> {
        let prepared = self.prepare()?;
        self.write_prepared(destination, &prepared)?;
        Ok(prepared.summary)
    }

    /// Writes a new file, flushes it, and reopens it for structural validation.
    ///
    /// Existing paths are never overwritten. A newly created partial file is
    /// removed when writing or validation fails.
    pub fn write_to_path(&mut self, path: impl AsRef<Path>) -> Result<WriteSummary> {
        let path = path.as_ref();
        let mut created = false;
        let result = (|| {
            let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
            created = true;
            let summary = self.write_to(&mut output)?;
            output.flush()?;
            output.sync_all()?;
            drop(output);
            self.validate_output_path(path)?;
            Ok(summary)
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(path);
        }
        result
    }

    fn prepare(&mut self) -> Result<PreparedWrite> {
        self.source.validate()?;
        if !self.database.connection().is_autocommit() {
            return Err(Error::InvalidWrite {
                reason: "editable database has an open transaction".to_owned(),
            });
        }
        self.database.quick_check()?;

        let chunks = self.source.chunks().collect::<Result<Vec<_>>>()?;
        let mut external_headers = BTreeMap::<u64, ExternalChunkHeader>::new();
        let mut identifiers = BTreeSet::<Vec<u8>>::new();
        for chunk in &chunks {
            if chunk.kind() != ChunkKind::External {
                continue;
            }
            let header = self.source.external_chunk_header(chunk)?;
            if !identifiers.insert(header.identifier().to_vec()) {
                return Err(Error::InvalidWrite {
                    reason: "source contains duplicate external identifiers".to_owned(),
                });
            }
            external_headers.insert(chunk.offset(), header);
        }
        for identifier in self.external_replacements.keys() {
            if !identifiers.contains(identifier) {
                return Err(Error::InvalidWrite {
                    reason: "replacement identifier does not exist in the source".to_owned(),
                });
            }
        }

        let mut output_offset = self.source.root_header().first_chunk_offset();
        let mut database_offset = None;
        let mut external_offsets = BTreeMap::<Vec<u8>, u64>::new();
        for chunk in &chunks {
            match chunk.kind() {
                ChunkKind::External => {
                    let header = external_headers.get(&chunk.offset()).ok_or_else(|| {
                        Error::InvalidWrite {
                            reason: "external chunk header was not prepared".to_owned(),
                        }
                    })?;
                    external_offsets.insert(header.identifier().to_vec(), output_offset);
                    let body_size = self
                        .external_replacements
                        .get(header.identifier())
                        .map_or(header.body_size(), |body| body.len() as u64);
                    let payload_size = 16_u64
                        .checked_add(header.identifier().len() as u64)
                        .and_then(|size| size.checked_add(body_size))
                        .ok_or(Error::OffsetOverflow)?;
                    output_offset = next_chunk_offset(output_offset, payload_size)?;
                }
                ChunkKind::Sqlite => {
                    database_offset = Some(output_offset);
                    break;
                }
                _ => {
                    output_offset = next_chunk_offset(output_offset, chunk.payload_size())?;
                }
            }
        }
        let database_offset = database_offset.ok_or(Error::InvalidWrite {
            reason: "source has no SQLite chunk".to_owned(),
        })?;

        let working = clone_database(self.database.connection())?;
        repair_external_offsets(&working, &external_offsets)?;
        quick_check_connection(&working)?;
        let database_bytes = working.serialize(MAIN_DB)?.to_vec();
        let database_size = database_bytes.len() as u64;
        let database_limit = self.source.limits().max_database_size();
        if database_size > database_limit {
            return Err(Error::LimitExceeded {
                resource: "serialized SQLite payload size",
                value: database_size,
                limit: database_limit,
            });
        }

        let mut output_file_size = self.source.root_header().first_chunk_offset();
        for chunk in &chunks {
            let payload_size = match chunk.kind() {
                ChunkKind::Header => 24_u64
                    .checked_add(self.source.file_header().identifier().len() as u64)
                    .ok_or(Error::OffsetOverflow)?,
                ChunkKind::External => {
                    let header = external_headers.get(&chunk.offset()).ok_or_else(|| {
                        Error::InvalidWrite {
                            reason: "external chunk header was not prepared".to_owned(),
                        }
                    })?;
                    let body_size = self
                        .external_replacements
                        .get(header.identifier())
                        .map_or(header.body_size(), |body| body.len() as u64);
                    16_u64
                        .checked_add(header.identifier().len() as u64)
                        .and_then(|size| size.checked_add(body_size))
                        .ok_or(Error::OffsetOverflow)?
                }
                ChunkKind::Sqlite => database_size,
                ChunkKind::Footer => 0,
                ChunkKind::Other(_) => {
                    return Err(Error::InvalidWrite {
                        reason: "strict rewrite unexpectedly encountered an unknown chunk"
                            .to_owned(),
                    });
                }
            };
            output_file_size = next_chunk_offset(output_file_size, payload_size)?;
        }

        let summary = WriteSummary {
            original_file_size: self.source.root_header().declared_file_size(),
            output_file_size,
            database_offset,
            database_payload_size: database_size,
            external_chunks: external_headers.len() as u64,
            replaced_external_bodies: self.external_replacements.len() as u64,
        };
        Ok(PreparedWrite {
            chunks,
            external_headers,
            database_bytes,
            summary,
        })
    }

    fn write_prepared<W: Write>(
        &mut self,
        destination: &mut W,
        prepared: &PreparedWrite,
    ) -> Result<()> {
        destination.write_all(ROOT_MAGIC)?;
        destination.write_all(&prepared.summary.output_file_size.to_be_bytes())?;
        destination.write_all(&self.source.root_header().first_chunk_offset().to_be_bytes())?;
        let gap_size = self
            .source
            .root_header()
            .first_chunk_offset()
            .checked_sub(ROOT_HEADER_SIZE)
            .ok_or(Error::OffsetOverflow)?;
        copy_range(
            &mut self.source.reader,
            ROOT_HEADER_SIZE,
            gap_size,
            destination,
        )?;

        for chunk in &prepared.chunks {
            match chunk.kind() {
                ChunkKind::Header => {
                    let identifier = self.source.file_header().identifier();
                    let payload_size = 24_u64
                        .checked_add(identifier.len() as u64)
                        .ok_or(Error::OffsetOverflow)?;
                    write_chunk_header(destination, FILE_HEADER_TAG, payload_size)?;
                    destination
                        .write_all(&self.source.file_header().format_version().to_be_bytes())?;
                    destination.write_all(&prepared.summary.database_offset.to_be_bytes())?;
                    destination.write_all(&(identifier.len() as u64).to_be_bytes())?;
                    destination.write_all(identifier)?;
                }
                ChunkKind::External => {
                    let header =
                        prepared
                            .external_headers
                            .get(&chunk.offset())
                            .ok_or_else(|| Error::InvalidWrite {
                                reason: "external chunk header was not prepared".to_owned(),
                            })?;
                    let replacement = self.external_replacements.get(header.identifier());
                    let body_size =
                        replacement.map_or(header.body_size(), |body| body.len() as u64);
                    let payload_size = 16_u64
                        .checked_add(header.identifier().len() as u64)
                        .and_then(|size| size.checked_add(body_size))
                        .ok_or(Error::OffsetOverflow)?;
                    write_chunk_header(destination, EXTERNAL_TAG, payload_size)?;
                    destination.write_all(&(header.identifier().len() as u64).to_be_bytes())?;
                    destination.write_all(header.identifier())?;
                    destination.write_all(&body_size.to_be_bytes())?;
                    if let Some(body) = replacement {
                        destination.write_all(body)?;
                    } else {
                        copy_range(
                            &mut self.source.reader,
                            header.body_offset(),
                            header.body_size(),
                            destination,
                        )?;
                    }
                }
                ChunkKind::Sqlite => {
                    write_chunk_header(
                        destination,
                        SQLITE_TAG,
                        prepared.database_bytes.len() as u64,
                    )?;
                    destination.write_all(&prepared.database_bytes)?;
                }
                ChunkKind::Footer => {
                    write_chunk_header(destination, FOOTER_TAG, 0)?;
                }
                ChunkKind::Other(_) => {
                    return Err(Error::InvalidWrite {
                        reason: "strict rewrite unexpectedly encountered an unknown chunk"
                            .to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_output_path(&self, path: &Path) -> Result<()> {
        let file = File::open(path)?;
        let mut output = ClipFile::open_with_limits(file, self.source.limits())?;
        let summary = output.validate()?;
        let database = output.open_database()?;
        database.quick_check()?;
        output.validate_external_index(&database)?;
        if output.file_header().format_version() != self.source.file_header().format_version()
            || output.file_header().identifier() != self.source.file_header().identifier()
            || output.root_header().declared_file_size()
                != summary.database_payload_size()
                    + output.file_header().database_offset()
                    + 2 * CHUNK_HEADER_SIZE
        {
            return Err(Error::InvalidWrite {
                reason: "rewritten container identity or size validation failed".to_owned(),
            });
        }
        Ok(())
    }
}

struct PreparedWrite {
    chunks: Vec<ChunkHeader>,
    external_headers: BTreeMap<u64, ExternalChunkHeader>,
    database_bytes: Vec<u8>,
    summary: WriteSummary,
}

fn open_editable_database<R: Read + Seek>(source: &mut ClipFile<R>) -> Result<Database> {
    let chunk = source.chunk_at_offset(source.file_header().database_offset())?;
    if chunk.kind() != ChunkKind::Sqlite {
        return Err(Error::InvalidWrite {
            reason: "CHNKHead database offset does not point to CHNKSQLi".to_owned(),
        });
    }
    let limit = source.limits().max_database_size();
    if chunk.payload_size() > limit {
        return Err(Error::LimitExceeded {
            resource: "SQLite payload size",
            value: chunk.payload_size(),
            limit,
        });
    }
    let size = usize::try_from(chunk.payload_size()).map_err(|_| Error::LimitExceeded {
        resource: "SQLite payload size",
        value: chunk.payload_size(),
        limit: usize::MAX as u64,
    })?;
    source
        .reader
        .seek(SeekFrom::Start(chunk.payload_offset()))?;
    let input = source.reader.by_ref().take(chunk.payload_size());
    let mut connection = Connection::open_in_memory()?;
    connection.deserialize_read_exact(MAIN_DB, input, size, false)?;
    Database::from_connection(connection)
}

fn clone_database(source: &Connection) -> Result<Connection> {
    let bytes = source.serialize(MAIN_DB)?;
    let mut clone = Connection::open_in_memory()?;
    clone.deserialize_read_exact(MAIN_DB, &*bytes, bytes.len(), false)?;
    Ok(clone)
}

fn repair_external_offsets(
    connection: &Connection,
    external_offsets: &BTreeMap<Vec<u8>, u64>,
) -> Result<()> {
    let row_count: i64 =
        connection.query_row("SELECT count(*) FROM ExternalChunk", [], |row| row.get(0))?;
    if row_count != i64::try_from(external_offsets.len()).map_err(|_| Error::OffsetOverflow)? {
        return Err(Error::InvalidWrite {
            reason: format!(
                "ExternalChunk contains {row_count} rows, expected {}",
                external_offsets.len()
            ),
        });
    }
    for (identifier, offset) in external_offsets {
        let offset = i64::try_from(*offset).map_err(|_| Error::OffsetOverflow)?;
        let current: Option<i64> = connection
            .query_row(
                "SELECT Offset FROM ExternalChunk \
                 WHERE CAST(ExternalID AS BLOB) = ?1",
                params![identifier],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Err(Error::InvalidWrite {
                reason: "ExternalChunk is missing a source identifier".to_owned(),
            });
        };
        if current == offset {
            continue;
        }
        let updated = connection.execute(
            "UPDATE ExternalChunk SET Offset = ?1 \
             WHERE CAST(ExternalID AS BLOB) = ?2",
            params![offset, identifier],
        )?;
        if updated != 1 {
            return Err(Error::InvalidWrite {
                reason: "ExternalChunk identifier is not unique".to_owned(),
            });
        }
    }
    Ok(())
}

fn quick_check_connection(connection: &Connection) -> Result<()> {
    let result: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(Error::InvalidWrite {
            reason: format!("SQLite quick_check failed: {result}"),
        })
    }
}

fn next_chunk_offset(offset: u64, payload_size: u64) -> Result<u64> {
    offset
        .checked_add(CHUNK_HEADER_SIZE)
        .and_then(|value| value.checked_add(payload_size))
        .ok_or(Error::OffsetOverflow)
}

fn write_chunk_header<W: Write>(
    destination: &mut W,
    tag: &[u8; 8],
    payload_size: u64,
) -> Result<()> {
    destination.write_all(tag)?;
    destination.write_all(&payload_size.to_be_bytes())?;
    Ok(())
}

fn copy_range<R: Read + Seek, W: Write>(
    source: &mut R,
    offset: u64,
    size: u64,
    destination: &mut W,
) -> Result<()> {
    source.seek(SeekFrom::Start(offset))?;
    let copied = io::copy(&mut source.take(size), destination)?;
    if copied != size {
        return Err(Error::InvalidWrite {
            reason: "source ended while copying preserved bytes".to_owned(),
        });
    }
    Ok(())
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use std::{
        io::Cursor,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    const EXTERNAL_ID: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";

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

    fn sample() -> Vec<u8> {
        let header_payload_size = 40_u64;
        let external_offset = ROOT_HEADER_SIZE + CHUNK_HEADER_SIZE + header_payload_size;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ExternalChunk (ExternalID BLOB NOT NULL, Offset INTEGER NOT NULL);
                 CREATE TABLE Metadata (Value TEXT NOT NULL);
                 INSERT INTO Metadata VALUES ('before');",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2)",
                params![EXTERNAL_ID, external_offset as i64],
            )
            .unwrap();
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();

        let mut external = Vec::new();
        push_u64(&mut external, EXTERNAL_ID.len() as u64);
        external.extend_from_slice(EXTERNAL_ID);
        push_u64(&mut external, 3);
        external.extend_from_slice(b"abc");
        let database_offset = external_offset + CHUNK_HEADER_SIZE + external.len() as u64;

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        push_u64(&mut header, database_offset);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);

        let mut bytes = Vec::from(*ROOT_MAGIC);
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, ROOT_HEADER_SIZE);
        push_chunk(&mut bytes, FILE_HEADER_TAG, &header);
        push_chunk(&mut bytes, EXTERNAL_TAG, &external);
        push_chunk(&mut bytes, SQLITE_TAG, &database);
        push_chunk(&mut bytes, FOOTER_TAG, b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        bytes
    }

    fn temp_output_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "clipfile-writer-test-{}-{nonce}.clip",
            std::process::id()
        ))
    }

    #[test]
    fn unchanged_rewrite_is_byte_exact() {
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source.clone())).unwrap();
        let mut writer = clip.writer().unwrap();
        let mut output = Vec::new();
        let summary = writer.write_to(&mut output).unwrap();
        assert_eq!(output, source);
        assert_eq!(summary.original_file_size(), summary.output_file_size());
        assert_eq!(summary.replaced_external_bodies(), 0);
    }

    #[test]
    fn rewrites_database_external_body_and_offsets() {
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        let transaction = writer
            .database_mut()
            .connection_mut()
            .transaction()
            .unwrap();
        transaction
            .execute("UPDATE Metadata SET Value = 'after'", [])
            .unwrap();
        transaction.commit().unwrap();
        writer
            .replace_external_body(EXTERNAL_ID, b"replacement".to_vec())
            .unwrap();

        let mut output = Vec::new();
        let summary = writer.write_to(&mut output).unwrap();
        assert_eq!(summary.replaced_external_bodies(), 1);
        assert!(summary.output_file_size() > summary.original_file_size());

        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        rewritten.validate().unwrap();
        let database = rewritten.open_database().unwrap();
        database.quick_check().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let value: String = database
            .connection()
            .query_row("SELECT Value FROM Metadata", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "after");
        let external = rewritten
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::External).then_some(chunk)
            })
            .unwrap();
        let payload = rewritten.read_chunk_payload(&external, 1024).unwrap();
        assert!(payload.ends_with(b"replacement"));
    }

    #[test]
    fn rejects_unknown_replacements_and_limits() {
        let source = sample();
        let mut clip = ClipFile::open_with_limits(
            Cursor::new(source),
            crate::Limits::default().with_max_write_external_body_size(2),
        )
        .unwrap();
        let mut writer = clip.writer().unwrap();
        assert!(matches!(
            writer.replace_external_body(EXTERNAL_ID, b"abc".to_vec()),
            Err(Error::LimitExceeded { .. })
        ));
        writer
            .replace_external_body(b"unknown", b"ok".to_vec())
            .unwrap();
        assert!(matches!(
            writer.write_to(&mut Vec::new()),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[test]
    fn rejects_an_open_database_transaction() {
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute_batch("BEGIN")
            .unwrap();

        assert!(matches!(
            writer.write_to(&mut Vec::new()),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[test]
    fn path_writer_never_overwrites_an_existing_file() {
        let path = temp_output_path();
        fs::write(&path, b"keep").unwrap();

        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        assert!(matches!(
            writer.write_to_path(&path),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn path_writer_creates_and_validates_a_new_file() {
        let path = temp_output_path();
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source.clone())).unwrap();
        let mut writer = clip.writer().unwrap();

        let summary = writer.write_to_path(&path).unwrap();
        assert_eq!(summary.output_file_size(), source.len() as u64);
        assert_eq!(fs::read(&path).unwrap(), source);
        fs::remove_file(path).unwrap();
    }
}
