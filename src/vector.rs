use std::io::{Read, Seek};

#[cfg(feature = "write")]
use rusqlite::OptionalExtension;
use rusqlite::{params, types::ValueRef};

use crate::{ClipFile, Database, Error, Limits, Result};
#[cfg(feature = "write")]
use crate::{ClipWriter, DatabaseSchema};

#[cfg(feature = "write")]
const SUPPORTED_VECTOR_HEADER_SIZE: usize = 92;
#[cfg(feature = "write")]
const SUPPORTED_VECTOR_POINT_SECTION_SIZE: u32 = 76;
#[cfg(feature = "write")]
const SUPPORTED_VECTOR_POINT_SIZE: usize = 88;

#[cfg(feature = "write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VectorStrokeRange {
    start: usize,
    end: usize,
    point_count: u32,
}

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

/// Result of translating every point in an existing supported vector body.
#[cfg(feature = "write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VectorTranslationSummary {
    strokes: u32,
    points: u32,
    delta_x: i32,
    delta_y: i32,
}

#[cfg(feature = "write")]
impl VectorTranslationSummary {
    /// Number of validated stroke records.
    #[must_use]
    pub const fn strokes(self) -> u32 {
        self.strokes
    }

    /// Number of translated points.
    #[must_use]
    pub const fn points(self) -> u32 {
        self.points
    }

    /// Horizontal translation in canvas units.
    #[must_use]
    pub const fn delta_x(self) -> i32 {
        self.delta_x
    }

    /// Vertical translation in canvas units.
    #[must_use]
    pub const fn delta_y(self) -> i32 {
        self.delta_y
    }
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

#[cfg(feature = "write")]
impl<R: Read + Seek> ClipWriter<'_, R> {
    /// Replaces the complete opaque body referenced by a validated vector row.
    ///
    /// The `VectorObjectList` row and external identifier must still match the
    /// supplied [`VectorDataSource`]. The body format is not interpreted, so
    /// this is intentionally an opaque boundary rather than a stroke encoder.
    /// Container offsets are repaired by [`Self::write_to`](ClipWriter::write_to).
    pub fn replace_vector_data_body(
        &mut self,
        source: &VectorDataSource,
        body: impl Into<Vec<u8>>,
        limits: Limits,
    ) -> Result<Option<Vec<u8>>> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = body.into();
        if body.len() as u64 > limits.max_vector_data_bytes() {
            return Err(Error::LimitExceeded {
                resource: "replacement vector data bytes",
                value: body.len() as u64,
                limit: limits.max_vector_data_bytes(),
            });
        }
        self.source_external_object(source.external_identifier())?;
        self.replace_external_body(source.external_identifier(), body)
    }

    /// Translates every point and bounding box in an existing vector body.
    ///
    /// This semantic edit is intentionally limited to the strictly validated
    /// 92-byte stroke header and 88-byte point layout observed in current
    /// generated documents. Unsupported layouts are rejected without
    /// installing a replacement, while brush, pressure, opacity, flags, and
    /// all unknown bytes are preserved exactly. Parsed stroke records are
    /// bounded by [`Limits::max_vector_objects`].
    pub fn translate_vector_data(
        &mut self,
        source: &VectorDataSource,
        delta_x: i32,
        delta_y: i32,
        limits: Limits,
    ) -> Result<VectorTranslationSummary> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        if body.len() as u64 > limits.max_vector_data_bytes() {
            return Err(Error::LimitExceeded {
                resource: "vector data bytes",
                value: body.len() as u64,
                limit: limits.max_vector_data_bytes(),
            });
        }
        let (translated, summary) =
            translate_supported_vector_body(&body, delta_x, delta_y, limits)?;
        if translated != body {
            self.replace_external_body(source.external_identifier(), translated)?;
        }
        Ok(summary)
    }

    /// Appends a translated clone of one existing supported vector stroke.
    ///
    /// `template_stroke_index` addresses a stroke in external-body order. The
    /// complete 92-byte header and every complete 88-byte point record are
    /// cloned, then only point positions and stroke/point bounding boxes are
    /// translated. Brush attributes, pressure, opacity, flags, and all opaque
    /// fields are retained byte-for-byte.
    ///
    /// The clone is appended to the same [`VectorDataSource`]. This does not
    /// synthesize a brush or a stroke header from zero. The returned index is
    /// the appended stroke's external-body order. Parsed stroke records are
    /// bounded by [`Limits::max_vector_objects`].
    pub fn clone_vector_stroke(
        &mut self,
        source: &VectorDataSource,
        template_stroke_index: usize,
        delta_x: i32,
        delta_y: i32,
        limits: Limits,
    ) -> Result<(usize, VectorTranslationSummary)> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        ensure_vector_body_within_limit(&body, limits, "vector data bytes")?;
        let (replacement, appended_index, summary) =
            clone_supported_vector_stroke(&body, template_stroke_index, delta_x, delta_y, limits)?;
        self.replace_external_body(source.external_identifier(), replacement)?;
        Ok((appended_index, summary))
    }

    /// Removes one existing supported vector stroke from external-body order.
    ///
    /// All bytes before and after the selected record are retained exactly.
    /// Removing the final stroke produces an empty external body. Saved render
    /// caches are separate external objects and are intentionally unchanged.
    /// Further semantic stroke operations reject that empty body until it is
    /// replaced with a supported body. Parsed stroke records are bounded by
    /// [`Limits::max_vector_objects`].
    pub fn remove_vector_stroke(
        &mut self,
        source: &VectorDataSource,
        stroke_index: usize,
        limits: Limits,
    ) -> Result<u32> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        ensure_vector_body_within_limit(&body, limits, "vector data bytes")?;
        let (replacement, removed_points) =
            remove_supported_vector_stroke(&body, stroke_index, limits)?;
        self.replace_external_body(source.external_identifier(), replacement)?;
        Ok(removed_points)
    }
}

#[cfg(feature = "write")]
fn translate_supported_vector_body(
    body: &[u8],
    delta_x: i32,
    delta_y: i32,
    limits: Limits,
) -> Result<(Vec<u8>, VectorTranslationSummary)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let mut output = body.to_vec();
    let mut points = 0_u32;
    for range in &ranges {
        translate_vector_bbox(&mut output, range.start + 24, delta_x, delta_y)?;
        for point_index in 0..range.point_count {
            let point_offset = range
                .start
                .checked_add(SUPPORTED_VECTOR_HEADER_SIZE)
                .and_then(|value| {
                    value.checked_add(
                        usize::try_from(point_index)
                            .ok()?
                            .checked_mul(SUPPORTED_VECTOR_POINT_SIZE)?,
                    )
                })
                .ok_or(Error::OffsetOverflow)?;
            translate_vector_f64(&mut output, point_offset, f64::from(delta_x))?;
            translate_vector_f64(&mut output, point_offset + 8, f64::from(delta_y))?;
            translate_vector_bbox(&mut output, point_offset + 16, delta_x, delta_y)?;
        }
        points = points
            .checked_add(range.point_count)
            .ok_or(Error::OffsetOverflow)?;
    }
    let strokes = u32::try_from(ranges.len()).map_err(|_| Error::OffsetOverflow)?;
    Ok((
        output,
        VectorTranslationSummary {
            strokes,
            points,
            delta_x,
            delta_y,
        },
    ))
}

#[cfg(feature = "write")]
fn supported_vector_stroke_ranges(body: &[u8], limits: Limits) -> Result<Vec<VectorStrokeRange>> {
    let mut ranges = Vec::new();
    let mut offset = 0_usize;
    while offset < body.len() {
        let header_size = read_vector_u32(body, offset)?;
        let point_section_size = read_vector_u32(body, offset + 4)?;
        let point_size = read_vector_u32(body, offset + 8)?;
        let default_point_size = read_vector_u32(body, offset + 12)?;
        if header_size != SUPPORTED_VECTOR_HEADER_SIZE as u32
            || point_section_size != SUPPORTED_VECTOR_POINT_SECTION_SIZE
            || point_size != SUPPORTED_VECTOR_POINT_SIZE as u32
            || default_point_size != SUPPORTED_VECTOR_POINT_SIZE as u32
        {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "unsupported vector layout at byte {offset}: header={header_size}, point-section={point_section_size}, point={point_size}, default-point={default_point_size}"
                ),
            });
        }
        let point_count = read_vector_u32(body, offset + 16)?;
        let points_size = usize::try_from(point_count)
            .map_err(|_| Error::OffsetOverflow)?
            .checked_mul(SUPPORTED_VECTOR_POINT_SIZE)
            .ok_or(Error::OffsetOverflow)?;
        let end = offset
            .checked_add(SUPPORTED_VECTOR_HEADER_SIZE)
            .and_then(|value| value.checked_add(points_size))
            .ok_or(Error::OffsetOverflow)?;
        if end > body.len() {
            return Err(Error::InvalidWrite {
                reason: format!("vector stroke at byte {offset} exceeds its external body"),
            });
        }
        let next_count = u64::try_from(ranges.len())
            .map_err(|_| Error::OffsetOverflow)?
            .checked_add(1)
            .ok_or(Error::OffsetOverflow)?;
        if next_count > limits.max_vector_objects() {
            return Err(Error::LimitExceeded {
                resource: "vector stroke records",
                value: next_count,
                limit: limits.max_vector_objects(),
            });
        }
        ranges.push(VectorStrokeRange {
            start: offset,
            end,
            point_count,
        });
        offset = end;
    }
    if ranges.is_empty() {
        return Err(Error::InvalidWrite {
            reason: "vector body contains no supported stroke records".to_owned(),
        });
    }
    Ok(ranges)
}

#[cfg(feature = "write")]
fn clone_supported_vector_stroke(
    body: &[u8],
    template_stroke_index: usize,
    delta_x: i32,
    delta_y: i32,
    limits: Limits,
) -> Result<(Vec<u8>, usize, VectorTranslationSummary)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let appended_index = ranges.len();
    let template = ranges
        .get(template_stroke_index)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!(
                "vector template stroke index {template_stroke_index} is out of range for {} strokes",
                ranges.len()
            ),
        })?;
    let resulting_count = u64::try_from(ranges.len())
        .map_err(|_| Error::OffsetOverflow)?
        .checked_add(1)
        .ok_or(Error::OffsetOverflow)?;
    if resulting_count > limits.max_vector_objects() {
        return Err(Error::LimitExceeded {
            resource: "vector stroke records after clone",
            value: resulting_count,
            limit: limits.max_vector_objects(),
        });
    }
    if template.point_count == 0 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector template stroke index {template_stroke_index} contains no points"
            ),
        });
    }
    let template_bytes = body
        .get(template.start..template.end)
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size = body
        .len()
        .checked_add(template_bytes.len())
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size_u64 =
        u64::try_from(replacement_size).map_err(|_| Error::OffsetOverflow)?;
    if replacement_size_u64 > limits.max_vector_data_bytes() {
        return Err(Error::LimitExceeded {
            resource: "replacement vector data bytes",
            value: replacement_size_u64,
            limit: limits.max_vector_data_bytes(),
        });
    }
    let (translated, summary) =
        translate_supported_vector_body(template_bytes, delta_x, delta_y, limits)?;
    let mut replacement = Vec::with_capacity(replacement_size);
    replacement.extend_from_slice(body);
    replacement.extend_from_slice(&translated);
    Ok((replacement, appended_index, summary))
}

#[cfg(feature = "write")]
fn remove_supported_vector_stroke(
    body: &[u8],
    stroke_index: usize,
    limits: Limits,
) -> Result<(Vec<u8>, u32)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let removed = ranges
        .get(stroke_index)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!(
                "vector stroke index {stroke_index} is out of range for {} strokes",
                ranges.len()
            ),
        })?;
    let removed_size = removed
        .end
        .checked_sub(removed.start)
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size = body
        .len()
        .checked_sub(removed_size)
        .ok_or(Error::OffsetOverflow)?;
    let mut replacement = Vec::with_capacity(replacement_size);
    replacement.extend_from_slice(&body[..removed.start]);
    replacement.extend_from_slice(&body[removed.end..]);
    Ok((replacement, removed.point_count))
}

#[cfg(feature = "write")]
fn ensure_vector_body_within_limit(
    body: &[u8],
    limits: Limits,
    resource: &'static str,
) -> Result<()> {
    if body.len() as u64 > limits.max_vector_data_bytes() {
        return Err(Error::LimitExceeded {
            resource,
            value: body.len() as u64,
            limit: limits.max_vector_data_bytes(),
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
fn read_vector_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset.checked_add(4).ok_or(Error::OffsetOverflow)?)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("vector record is truncated at byte {offset}"),
        })?;
    Ok(u32::from_be_bytes(
        value.try_into().expect("four-byte slice"),
    ))
}

#[cfg(feature = "write")]
fn translate_vector_f64(bytes: &mut [u8], offset: usize, delta: f64) -> Result<()> {
    let end = offset.checked_add(8).ok_or(Error::OffsetOverflow)?;
    let value = bytes.get(offset..end).ok_or_else(|| Error::InvalidWrite {
        reason: format!("vector coordinate is truncated at byte {offset}"),
    })?;
    let original = f64::from_be_bytes(value.try_into().expect("eight-byte slice"));
    let replacement = original + delta;
    if !original.is_finite() || !replacement.is_finite() {
        return Err(Error::InvalidWrite {
            reason: format!("vector coordinate at byte {offset} is not finite"),
        });
    }
    bytes[offset..end].copy_from_slice(&replacement.to_be_bytes());
    Ok(())
}

#[cfg(feature = "write")]
fn translate_vector_bbox(
    bytes: &mut [u8],
    offset: usize,
    delta_x: i32,
    delta_y: i32,
) -> Result<()> {
    for (index, delta) in [delta_x, delta_y, delta_x, delta_y].into_iter().enumerate() {
        let value_offset = offset
            .checked_add(index.checked_mul(4).ok_or(Error::OffsetOverflow)?)
            .ok_or(Error::OffsetOverflow)?;
        let original = i32::from_be_bytes(
            bytes
                .get(value_offset..value_offset + 4)
                .ok_or_else(|| Error::InvalidWrite {
                    reason: format!("vector bounding box is truncated at byte {value_offset}"),
                })?
                .try_into()
                .expect("four-byte slice"),
        );
        let replacement = original
            .checked_add(delta)
            .ok_or_else(|| Error::InvalidWrite {
                reason: "vector bounding-box translation overflows i32".to_owned(),
            })?;
        bytes[value_offset..value_offset + 4].copy_from_slice(&replacement.to_be_bytes());
    }
    Ok(())
}

#[cfg(feature = "write")]
fn validate_vector_source(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    source: &VectorDataSource,
) -> Result<()> {
    for column in ["MainId", "CanvasId", "LayerId", "VectorData"] {
        if !schema.has_column("VectorObjectList", column) {
            return Err(Error::InvalidWrite {
                reason: format!("VectorObjectList.{column} is required to edit vector data"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM VectorObjectList WHERE MainId = ?1",
        params![source.id()],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "expected one vector row with ID {}, found {row_count}",
                source.id()
            ),
        });
    }
    let stored = connection
        .query_row(
            "SELECT CanvasId, LayerId, VectorData FROM VectorObjectList \
             WHERE MainId = ?1 LIMIT 1",
            params![source.id()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    match row.get_ref(2)? {
                        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes.to_vec(),
                        value => {
                            return Err(rusqlite::Error::InvalidColumnType(
                                2,
                                "VectorData".to_owned(),
                                value.data_type(),
                            ));
                        }
                    },
                ))
            },
        )
        .optional()?;
    let Some((canvas_id, layer_id, identifier)) = stored else {
        return Err(Error::InvalidWrite {
            reason: format!("vector row {} does not exist", source.id()),
        });
    };
    if canvas_id != source.canvas_id()
        || layer_id != source.layer_id()
        || identifier != source.external_identifier()
    {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector row {} changed after VectorDataSource was read",
                source.id()
            ),
        });
    }
    let alias_count: i64 = connection.query_row(
        "SELECT count(*) FROM VectorObjectList WHERE CAST(VectorData AS BLOB) = ?1",
        params![&identifier],
        |row| row.get(0),
    )?;
    if alias_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector external identifier is shared by {alias_count} VectorObjectList rows"
            ),
        });
    }
    Ok(())
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
    #[cfg(feature = "write")]
    use rusqlite::MAIN_DB;

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

    #[cfg(feature = "write")]
    fn writable_sample(body: &[u8]) -> Vec<u8> {
        let external_offset = 24 + 16 + 40;
        let external_chunk_size = 16 + 16 + IDENTIFIER.len() as u64 + body.len() as u64;
        let database_offset = external_offset + external_chunk_size;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE VectorObjectList (
                    MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
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
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        push_u64(&mut header, database_offset);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);
        let mut external = Vec::new();
        push_u64(&mut external, IDENTIFIER.len() as u64);
        external.extend_from_slice(IDENTIFIER);
        push_u64(&mut external, body.len() as u64);
        external.extend_from_slice(body);

        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);
        assert_eq!(push_chunk(&mut bytes, b"CHNKHead", &header), 24);
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKExta", &external),
            external_offset
        );
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKSQLi", &database),
            database_offset
        );
        push_chunk(&mut bytes, b"CHNKFoot", b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        bytes
    }

    #[cfg(feature = "write")]
    fn supported_vector_body() -> Vec<u8> {
        let mut body = vec![0_u8; 92 + 2 * 88];
        for (offset, value) in [(0, 92_u32), (4, 76), (8, 88), (12, 88), (16, 2)] {
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        body[20..24].copy_from_slice(&0x2081_u32.to_be_bytes());
        for (index, value) in [5_i32, 148, 76, 197].into_iter().enumerate() {
            let offset = 24 + index * 4;
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (point, (x, y, bbox)) in [
            (10.0_f64, 152.5_f64, [5_i32, 148, 76, 197]),
            (70.0_f64, 191.0_f64, [65_i32, 186, 75, 196]),
        ]
        .into_iter()
        .enumerate()
        {
            let offset = 92 + point * 88;
            body[offset..offset + 8].copy_from_slice(&x.to_be_bytes());
            body[offset + 8..offset + 16].copy_from_slice(&y.to_be_bytes());
            for (index, value) in bbox.into_iter().enumerate() {
                let bbox_offset = offset + 16 + index * 4;
                body[bbox_offset..bbox_offset + 4].copy_from_slice(&value.to_be_bytes());
            }
        }
        body
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

    #[cfg(feature = "write")]
    #[test]
    fn replaces_a_validated_vector_body_and_reads_it_back() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        assert!(
            writer
                .replace_vector_data_body(&source, b"new opaque vector".to_vec(), Limits::default())
                .unwrap()
                .is_none()
        );

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert_eq!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap(),
            b"new opaque vector"
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn writes_a_vector_identifier_stored_with_the_text_storage_class() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "UPDATE VectorObjectList \
                 SET VectorData = CAST(VectorData AS TEXT) WHERE MainId = 1",
                [],
            )
            .unwrap();
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT typeof(VectorData) FROM VectorObjectList WHERE MainId = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "text"
        );

        assert!(writer
            .replace_vector_data_body(&source, b"new opaque vector".to_vec(), Limits::default(),)
            .is_ok());
        assert_eq!(writer.replacement_count(), 1);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_a_vector_identifier_shared_by_multiple_rows() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "INSERT INTO VectorObjectList VALUES (2, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();

        assert!(matches!(
            writer.replace_vector_data_body(
                &source,
                b"new opaque vector".to_vec(),
                Limits::default()
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn translates_supported_vector_points_and_bounding_boxes() {
        let body = supported_vector_body();
        let (translated, summary) =
            translate_supported_vector_body(&body, 3, -4, Limits::default()).unwrap();
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(summary.delta_x(), 3);
        assert_eq!(summary.delta_y(), -4);
        assert_eq!(
            f64::from_be_bytes(translated[92..100].try_into().unwrap()),
            13.0
        );
        assert_eq!(
            f64::from_be_bytes(translated[100..108].try_into().unwrap()),
            148.5
        );
        assert_eq!(
            i32::from_be_bytes(translated[24..28].try_into().unwrap()),
            8
        );
        assert_eq!(
            i32::from_be_bytes(translated[28..32].try_into().unwrap()),
            144
        );
        for index in 0..body.len() {
            let is_coordinate = matches!(
                index,
                24..=39 | 92..=123 | 180..=211
            );
            if !is_coordinate {
                assert_eq!(translated[index], body[index], "byte {index} changed");
            }
        }
    }

    #[cfg(feature = "write")]
    #[test]
    fn clones_one_supported_vector_stroke_and_preserves_opaque_bytes() {
        let mut body = supported_vector_body();
        body[40..52].copy_from_slice(&[0xA5; 12]);
        body[72..92].copy_from_slice(&[0x5A; 20]);
        body[124..180].copy_from_slice(&[0xC3; 56]);
        body[212..268].copy_from_slice(&[0x3C; 56]);

        let (replacement, appended_index, summary) =
            clone_supported_vector_stroke(&body, 0, 3, -4, Limits::default()).unwrap();
        assert_eq!(appended_index, 1);
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(&replacement[..body.len()], body.as_slice());
        let clone = &replacement[body.len()..];
        assert_eq!(clone.len(), body.len());
        assert_eq!(f64::from_be_bytes(clone[92..100].try_into().unwrap()), 13.0);
        assert_eq!(
            f64::from_be_bytes(clone[100..108].try_into().unwrap()),
            148.5
        );
        assert_eq!(i32::from_be_bytes(clone[24..28].try_into().unwrap()), 8);
        assert_eq!(i32::from_be_bytes(clone[28..32].try_into().unwrap()), 144);
        for index in 0..body.len() {
            let is_coordinate = matches!(index, 24..=39 | 92..=123 | 180..=211);
            if !is_coordinate {
                assert_eq!(clone[index], body[index], "byte {index} changed");
            }
        }
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_only_the_selected_supported_vector_record() {
        let mut first = supported_vector_body();
        first[40] = 0x11;
        let mut second = supported_vector_body();
        second[40] = 0x22;
        let mut third = supported_vector_body();
        third[40] = 0x33;
        let body = [&first[..], &second[..], &third[..]].concat();

        let (replacement, removed_points) =
            remove_supported_vector_stroke(&body, 1, Limits::default()).unwrap();
        assert_eq!(removed_points, 2);
        assert_eq!(replacement, [&first[..], &third[..]].concat());
        assert_eq!(
            supported_vector_stroke_ranges(&replacement, Limits::default())
                .unwrap()
                .len(),
            2
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_final_vector_stroke_and_rejects_missing_index() {
        let mut clip =
            ClipFile::open(Cursor::new(writable_sample(&supported_vector_body()))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 1, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(
            writer
                .remove_vector_stroke(&source, 0, Limits::default())
                .unwrap(),
            2
        );
        assert_eq!(writer.replacement_count(), 1);
        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.translate_vector_data(&source, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(writer.addition_count(), 0);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_vector_stroke_count_limit_without_a_replacement() {
        let stroke = supported_vector_body();
        let body = [&stroke[..], &stroke[..]].concat();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        let limits = Limits::default().with_max_vector_objects(1);

        for result in [
            writer
                .translate_vector_data(&source, 0, 0, limits)
                .map(|_| ()),
            writer
                .clone_vector_stroke(&source, 0, 0, 0, limits)
                .map(|_| ()),
            writer.remove_vector_stroke(&source, 0, limits).map(|_| ()),
        ] {
            assert!(matches!(
                result,
                Err(Error::LimitExceeded {
                    resource: "vector stroke records",
                    value: 2,
                    limit: 1,
                })
            ));
        }
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_clone_stroke_count_limit_without_a_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(
                &source,
                0,
                0,
                0,
                Limits::default().with_max_vector_objects(1)
            ),
            Err(Error::LimitExceeded {
                resource: "vector stroke records after clone",
                value: 2,
                limit: 1,
            })
        ));
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_clone_output_limit_without_a_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        let limit = (body.len() * 2 - 1) as u64;

        assert!(matches!(
            writer.clone_vector_stroke(
                &source,
                0,
                0,
                0,
                Limits::default().with_max_vector_data_bytes(limit)
            ),
            Err(Error::LimitExceeded {
                resource: "replacement vector data bytes",
                ..
            })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_truncated_clone_template_without_a_replacement() {
        let mut body = supported_vector_body();
        body.pop();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn clone_and_remove_reject_shared_vector_identifier_without_a_replacement() {
        let mut clip =
            ClipFile::open(Cursor::new(writable_sample(&supported_vector_body()))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "INSERT INTO VectorObjectList VALUES (2, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 1, 1, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn clone_then_remove_round_trips_through_the_writer() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        let (appended_index, summary) = writer
            .clone_vector_stroke(&source, 0, 3, -4, Limits::default())
            .unwrap();
        assert_eq!(appended_index, 1);
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(
            writer
                .remove_vector_stroke(&source, 1, Limits::default())
                .unwrap(),
            2
        );

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert_eq!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap(),
            body
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn failed_follow_up_edit_keeps_the_previous_vector_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .clone_vector_stroke(&source, 0, 3, -4, Limits::default())
            .unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 99, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 99, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(writer.addition_count(), 0);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let rewritten_body = rewritten
            .read_vector_data(&database, &source, Limits::default())
            .unwrap();
        assert_eq!(
            supported_vector_stroke_ranges(&rewritten_body, Limits::default())
                .unwrap()
                .len(),
            2
        );
        assert_eq!(&rewritten_body[..body.len()], body.as_slice());
    }
}
