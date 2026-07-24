use std::{collections::BTreeMap, io::Read, io::Seek, io::SeekFrom};

use rusqlite::{Connection, MAIN_DB, params, types::ValueRef};

use crate::{ChunkKind, ClipFile, Error, ExternalObject, Result};

/// Metadata for one SQLite column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnSchema {
    ordinal: u32,
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: u32,
    hidden: u32,
}

impl ColumnSchema {
    /// Zero-based ordinal within the table.
    #[must_use]
    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    /// Column name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Declared SQLite type, which may be empty.
    #[must_use]
    pub fn declared_type(&self) -> &str {
        &self.declared_type
    }

    /// Whether SQLite declares the column as `NOT NULL`.
    #[must_use]
    pub const fn is_not_null(&self) -> bool {
        self.not_null
    }

    /// SQL expression used as the default value.
    #[must_use]
    pub fn default_value(&self) -> Option<&str> {
        self.default_value.as_deref()
    }

    /// One-based primary-key position, or zero when not in the key.
    #[must_use]
    pub const fn primary_key_position(&self) -> u32 {
        self.primary_key_position
    }

    /// SQLite `table_xinfo` hidden/generated-column value.
    #[must_use]
    pub const fn hidden(&self) -> u32 {
        self.hidden
    }
}

/// Runtime schema information for one table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableSchema {
    name: String,
    columns: Vec<ColumnSchema>,
}

impl TableSchema {
    /// Table name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Columns in SQLite ordinal order.
    #[must_use]
    pub fn columns(&self) -> &[ColumnSchema] {
        &self.columns
    }

    /// Looks up a column by its exact name.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&ColumnSchema> {
        self.columns.iter().find(|column| column.name == name)
    }
}

/// Runtime schema of the embedded database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseSchema {
    tables: BTreeMap<String, TableSchema>,
}

impl DatabaseSchema {
    /// Tables ordered lexicographically by name.
    pub fn tables(&self) -> impl ExactSizeIterator<Item = &TableSchema> {
        self.tables.values()
    }

    /// Looks up a table by exact name.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    /// Returns whether the table and column both exist.
    #[must_use]
    pub fn has_column(&self, table: &str, column: &str) -> bool {
        self.table(table)
            .and_then(|schema| schema.column(column))
            .is_some()
    }
}

/// One row from the SQLite `ExternalChunk` index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalChunkRecord {
    identifier: Box<[u8]>,
    offset: u64,
}

impl ExternalChunkRecord {
    /// External identifier bytes.
    #[must_use]
    pub fn identifier(&self) -> &[u8] {
        &self.identifier
    }

    /// Absolute `CHNKExta` chunk-header offset.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }
}

/// A table and column declared to contain external-object identifiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalReferenceColumn {
    table: String,
    column: String,
}

impl ExternalReferenceColumn {
    /// Declared table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Declared column name.
    #[must_use]
    pub fn column(&self) -> &str {
        &self.column
    }
}

/// Read-only access to the embedded SQLite database.
pub struct Database {
    connection: Connection,
    schema: DatabaseSchema,
}

impl Database {
    pub(crate) fn from_connection(connection: Connection) -> Result<Self> {
        let schema = read_schema(&connection)?;
        Ok(Self { connection, schema })
    }

    /// Runtime schema discovered from this specific file.
    #[must_use]
    pub const fn schema(&self) -> &DatabaseSchema {
        &self.schema
    }

    /// Underlying read-only rusqlite connection for advanced queries.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Runs SQLite's bounded quick integrity check.
    pub fn quick_check(&self) -> Result<()> {
        let result: String = self
            .connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if result == "ok" {
            Ok(())
        } else {
            Err(Error::InvalidDatabase { reason: result })
        }
    }

    /// Counts rows in a table after validating its name against the schema.
    pub fn row_count(&self, table: &str) -> Result<u64> {
        self.require_table(table)?;
        let sql = format!("SELECT count(*) FROM {}", quote_identifier(table));
        let count: i64 = self.connection.query_row(&sql, [], |row| row.get(0))?;
        u64::try_from(count).map_err(|_| Error::InvalidDatabase {
            reason: format!("negative row count for table {table:?}"),
        })
    }

    /// Reads the external identifier-to-offset index.
    pub fn external_chunks(&self) -> Result<Vec<ExternalChunkRecord>> {
        self.require_column("ExternalChunk", "ExternalID")?;
        self.require_column("ExternalChunk", "Offset")?;
        let mut statement = self
            .connection
            .prepare("SELECT ExternalID, Offset FROM ExternalChunk ORDER BY Offset")?;
        let rows = statement.query_map([], |row| {
            let value = row.get_ref(0)?;
            let identifier = match value {
                ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Box::from(bytes),
                _ => {
                    return Err(rusqlite::Error::InvalidColumnType(
                        0,
                        "ExternalID".to_owned(),
                        value.data_type(),
                    ));
                }
            };
            let offset: i64 = row.get(1)?;
            Ok((identifier, offset))
        })?;
        rows.map(|row| {
            let (identifier, offset) = row?;
            let offset = u64::try_from(offset).map_err(|_| Error::InvalidDatabase {
                reason: "ExternalChunk contains a negative offset".to_owned(),
            })?;
            Ok(ExternalChunkRecord { identifier, offset })
        })
        .collect()
    }

    /// Looks up one external-object index row by its opaque identifier.
    pub fn external_chunk(&self, identifier: &[u8]) -> Result<Option<ExternalChunkRecord>> {
        self.require_column("ExternalChunk", "ExternalID")?;
        self.require_column("ExternalChunk", "Offset")?;
        let mut statement = self.connection.prepare(
            "SELECT ExternalID, Offset FROM ExternalChunk \
             WHERE CAST(ExternalID AS BLOB) = ?1 LIMIT 1",
        )?;
        let mut rows = statement.query(params![identifier])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let identifier = match row.get_ref(0)? {
            ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Box::from(bytes),
            value => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "ExternalID".to_owned(),
                    value.data_type(),
                )
                .into());
            }
        };
        let raw_offset: i64 = row.get(1)?;
        let offset = u64::try_from(raw_offset).map_err(|_| Error::InvalidDatabase {
            reason: "ExternalChunk contains a negative offset".to_owned(),
        })?;
        Ok(Some(ExternalChunkRecord { identifier, offset }))
    }

    /// Reads declarations of columns that may reference external objects.
    pub fn external_reference_columns(&self) -> Result<Vec<ExternalReferenceColumn>> {
        self.require_column("ExternalTableAndColumnName", "TableName")?;
        self.require_column("ExternalTableAndColumnName", "ColumnName")?;
        let mut statement = self.connection.prepare(
            "SELECT TableName, ColumnName FROM ExternalTableAndColumnName ORDER BY rowid",
        )?;
        statement
            .query_map([], |row| {
                Ok(ExternalReferenceColumn {
                    table: row.get(0)?,
                    column: row.get(1)?,
                })
            })?
            .map(|row| row.map_err(Error::from))
            .collect()
    }

    pub(crate) fn require_table(&self, table: &str) -> Result<&TableSchema> {
        self.schema.table(table).ok_or_else(|| Error::MissingTable {
            table: table.to_owned(),
        })
    }

    pub(crate) fn require_column(&self, table: &str, column: &str) -> Result<&ColumnSchema> {
        self.require_table(table)?
            .column(column)
            .ok_or_else(|| Error::MissingColumn {
                table: table.to_owned(),
                column: column.to_owned(),
            })
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Resolves one SQLite external-object identifier to its validated chunk.
    ///
    /// `None` means the identifier is not present in `ExternalChunk`.
    pub fn resolve_external_object(
        &mut self,
        database: &Database,
        identifier: &[u8],
    ) -> Result<Option<ExternalObject>> {
        let Some(record) = database.external_chunk(identifier)? else {
            return Ok(None);
        };
        let chunk = self.chunk_at_offset(record.offset())?;
        if chunk.kind() != ChunkKind::External {
            return Err(Error::InvalidDatabase {
                reason: format!(
                    "external index offset {} does not point to CHNKExta",
                    record.offset()
                ),
            });
        }
        let object = self.inspect_external_chunk(&chunk)?;
        if object.header().identifier() != identifier {
            return Err(Error::InvalidDatabase {
                reason: format!(
                    "external identifier does not match the chunk at offset {}",
                    record.offset()
                ),
            });
        }
        Ok(Some(object))
    }

    /// Opens the embedded SQLite payload as a read-only in-memory database.
    ///
    /// This method is available with the `sqlite` feature. The database is
    /// copied directly from the bounded chunk reader into SQLite-managed
    /// memory without first allocating a second complete `Vec<u8>`.
    pub fn open_database(&mut self) -> Result<Database> {
        let database_offset = self.file_header().database_offset();
        let max_database_size = self.limits.max_database_size();
        let mut database_chunk = None;
        for chunk in self.chunks() {
            let chunk = chunk?;
            if chunk.kind() == ChunkKind::Sqlite {
                database_chunk = Some(chunk);
                break;
            }
        }
        let chunk = database_chunk.ok_or(Error::InvalidChunkSequence {
            reason: "missing SQLite chunk",
        })?;
        if chunk.offset() != database_offset {
            return Err(Error::InvalidChunkSequence {
                reason: "SQLite offset does not match CHNKHead",
            });
        }
        if chunk.payload_size() > max_database_size {
            return Err(Error::LimitExceeded {
                resource: "SQLite payload size",
                value: chunk.payload_size(),
                limit: max_database_size,
            });
        }
        let size = usize::try_from(chunk.payload_size()).map_err(|_| Error::LimitExceeded {
            resource: "SQLite payload size",
            value: chunk.payload_size(),
            limit: usize::MAX as u64,
        })?;
        self.reader.seek(SeekFrom::Start(chunk.payload_offset()))?;
        let source = self.reader.by_ref().take(chunk.payload_size());
        let mut connection = Connection::open_in_memory()?;
        connection.deserialize_read_exact(MAIN_DB, source, size, true)?;
        Database::from_connection(connection)
    }

    /// Verifies that SQLite's external-object index matches the container.
    ///
    /// Every `ExternalChunk` row must point to a `CHNKExta` header with the
    /// same identifier, and every external chunk must have exactly one row.
    pub fn validate_external_index(&mut self, database: &Database) -> Result<()> {
        let records = database.external_chunks()?;
        let chunks = self
            .chunks()
            .filter_map(|chunk| match chunk {
                Ok(chunk) if chunk.kind() == ChunkKind::External => Some(Ok(chunk)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        if records.len() != chunks.len() {
            return Err(Error::InvalidDatabase {
                reason: format!(
                    "ExternalChunk has {} rows, but the container has {} CHNKExta chunks",
                    records.len(),
                    chunks.len()
                ),
            });
        }
        for record in records {
            let chunk = chunks
                .iter()
                .find(|chunk| chunk.offset() == record.offset())
                .ok_or_else(|| Error::InvalidDatabase {
                    reason: format!(
                        "ExternalChunk offset {} does not point to CHNKExta",
                        record.offset()
                    ),
                })?;
            let header = self.external_chunk_header(chunk)?;
            if header.identifier() != record.identifier() {
                return Err(Error::InvalidDatabase {
                    reason: format!(
                        "external identifier at offset {} does not match SQLite",
                        record.offset()
                    ),
                });
            }
        }
        Ok(())
    }
}

fn read_schema(connection: &Connection) -> Result<DatabaseSchema> {
    let names = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut tables = BTreeMap::new();
    for name in names {
        let mut statement = connection.prepare(
            "SELECT cid, name, type, \"notnull\", dflt_value, pk, hidden \
             FROM pragma_table_xinfo(?1) ORDER BY cid",
        )?;
        let raw_columns = statement
            .query_map(params![name], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let columns = raw_columns
            .into_iter()
            .map(
                |(
                    ordinal,
                    column_name,
                    declared_type,
                    not_null,
                    default_value,
                    primary_key_position,
                    hidden,
                )| {
                    Ok(ColumnSchema {
                        ordinal: sqlite_u32("column ordinal", ordinal)?,
                        name: column_name,
                        declared_type,
                        not_null: not_null != 0,
                        default_value,
                        primary_key_position: sqlite_u32(
                            "primary-key position",
                            primary_key_position,
                        )?,
                        hidden: sqlite_u32("hidden-column value", hidden)?,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        tables.insert(name.clone(), TableSchema { name, columns });
    }
    Ok(DatabaseSchema { tables })
}

fn sqlite_u32(field: &str, value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::InvalidDatabase {
        reason: format!("{field} is outside the u32 range: {value}"),
    })
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Canvas (MainId INTEGER, CanvasWidth REAL); \
                 INSERT INTO Canvas VALUES (1, 640.0); \
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER); \
                 INSERT INTO ExternalChunk VALUES (x'616263', 80); \
                 CREATE TABLE ExternalTableAndColumnName \
                    (TableName TEXT, ColumnName TEXT); \
                 INSERT INTO ExternalTableAndColumnName \
                    VALUES ('Offscreen', 'BlockData');",
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn discovers_schema_by_name() {
        let database = database();
        assert!(database.schema().has_column("Canvas", "CanvasWidth"));
        assert!(!database.schema().has_column("Canvas", "CanvasHeight"));
        assert_eq!(database.row_count("Canvas").unwrap(), 1);
        assert!(matches!(
            database.row_count("Missing"),
            Err(Error::MissingTable { .. })
        ));
    }

    #[test]
    fn reads_external_indexes() {
        let database = database();
        let chunks = database.external_chunks().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].identifier(), b"abc");
        assert_eq!(chunks[0].offset(), 80);
        let references = database.external_reference_columns().unwrap();
        assert_eq!(references[0].table(), "Offscreen");
        assert_eq!(references[0].column(), "BlockData");
        database.quick_check().unwrap();
    }
}
