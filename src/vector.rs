use std::io::{Read, Seek};

use rusqlite::{params, types::ValueRef};

use crate::{ClipFile, Database, Error, Limits, Result};

/// One `VectorObjectList` row and its opaque external-object identifier.
///
/// The vector body format is not interpreted yet. This type preserves enough
/// metadata to retrieve the bounded raw bytes without guessing its structure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VectorDataSource {
    id: i64,
    canvas_id: i64,
    layer_id: i64,
    external_identifier: Box<[u8]>,
}

impl VectorDataSource {
    /// `VectorObjectList.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Owning layer ID.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Opaque identifier used by the SQLite external-object index.
    #[must_use]
    pub fn external_identifier(&self) -> &[u8] {
        &self.external_identifier
    }
}

impl Database {
    /// Reads all vector-data references owned by one layer.
    ///
    /// Files without a `VectorObjectList` table return an empty vector. The
    /// raw external body can be obtained with [`ClipFile::read_vector_data`].
    pub fn vector_data_sources(
        &self,
        layer_id: i64,
        limits: Limits,
    ) -> Result<Vec<VectorDataSource>> {
        if self.schema().table("VectorObjectList").is_none() {
            return Ok(Vec::new());
        }
        for column in ["MainId", "CanvasId", "LayerId", "VectorData"] {
            self.require_column("VectorObjectList", column)?;
        }
        let mut statement = self.connection().prepare(
            "SELECT MainId, CanvasId, LayerId, VectorData \
             FROM VectorObjectList WHERE LayerId = ?1 ORDER BY MainId",
        )?;
        let mut rows = statement.query(params![layer_id])?;
        let mut sources = Vec::new();
        while let Some(row) = rows.next()? {
            let next_count = u64::try_from(sources.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            if next_count > limits.max_vector_objects() {
                return Err(Error::LimitExceeded {
                    resource: "vector objects per layer",
                    value: next_count,
                    limit: limits.max_vector_objects(),
                });
            }
            sources.push(VectorDataSource {
                id: row.get(0)?,
                canvas_id: row.get(1)?,
                layer_id: row.get(2)?,
                external_identifier: value_bytes(
                    row.get_ref(3)?,
                    3,
                    "VectorData",
                    limits.max_identifier_size(),
                )?,
            });
        }
        Ok(sources)
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Resolves and reads one opaque vector-data external body under a limit.
    pub fn read_vector_data(
        &mut self,
        database: &Database,
        source: &VectorDataSource,
        limits: Limits,
    ) -> Result<Vec<u8>> {
        let identifier = source.external_identifier();
        let object = self
            .resolve_external_object(database, identifier)?
            .ok_or_else(|| Error::InvalidDatabase {
                reason: format!(
                    "VectorObjectList row {} references a missing external object",
                    source.id()
                ),
            })?;
        self.read_external_body(&object, limits.max_vector_data_bytes())
    }
}

fn value_bytes(value: ValueRef<'_>, column: usize, name: &str, limit: u64) -> Result<Box<[u8]>> {
    let bytes = match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes,
        value => {
            return Err(rusqlite::Error::InvalidColumnType(
                column,
                name.to_owned(),
                value.data_type(),
            )
            .into());
        }
    };
    let size = bytes.len() as u64;
    if size > limit {
        return Err(Error::LimitExceeded {
            resource: "vector external identifier size",
            value: size,
            limit,
        });
    }
    Ok(Box::from(bytes))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use rusqlite::Connection;

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

    fn database(external_offset: u64) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE VectorObjectList (
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    VectorData BLOB
                 );
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO VectorObjectList VALUES (1, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2)",
                params![IDENTIFIER, external_offset as i64],
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn resolves_and_reads_raw_vector_data() {
        let body = b"opaque vector body";
        let (bytes, offset) = sample(body);
        let database = database(offset);
        let sources = database.vector_data_sources(3, Limits::default()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id(), 1);
        assert_eq!(sources[0].canvas_id(), 2);
        assert_eq!(sources[0].layer_id(), 3);
        assert_eq!(sources[0].external_identifier(), IDENTIFIER);

        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert_eq!(
            clip.read_vector_data(&database, &sources[0], Limits::default())
                .unwrap(),
            body
        );
    }

    #[test]
    fn enforces_vector_limits_and_tolerates_missing_table() {
        let connection = Connection::open_in_memory().unwrap();
        let empty = Database::from_connection(connection).unwrap();
        assert!(
            empty
                .vector_data_sources(3, Limits::default())
                .unwrap()
                .is_empty()
        );

        let body = b"1234";
        let (bytes, offset) = sample(body);
        let database = database(offset);
        assert!(matches!(
            database.vector_data_sources(3, Limits::default().with_max_vector_objects(0)),
            Err(Error::LimitExceeded { .. })
        ));
        assert!(matches!(
            database.vector_data_sources(
                3,
                Limits::default().with_max_identifier_size((IDENTIFIER.len() - 1) as u64)
            ),
            Err(Error::LimitExceeded { .. })
        ));
        let source = database.vector_data_sources(3, Limits::default()).unwrap();
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            clip.read_vector_data(
                &database,
                &source[0],
                Limits::default().with_max_vector_data_bytes(3)
            ),
            Err(Error::PayloadTooLarge { size: 4, limit: 3 })
        ));
    }
}
