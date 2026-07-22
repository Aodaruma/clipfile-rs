use std::str;

use rusqlite::{OptionalExtension, params, types::ValueRef};

use crate::{Database, Error, Limits, Result};

/// One text object stored in a CLIP text layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextObjectData {
    text: String,
    attributes: Box<[u8]>,
}

impl TextObjectData {
    /// UTF-8 text content.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Opaque `TextLayerAttributes` bytes paired with this text object.
    #[must_use]
    pub fn attributes(&self) -> &[u8] {
        &self.attributes
    }
}

/// Text-specific data read from one `Layer` row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextLayerData {
    layer_id: i64,
    layer_type: i64,
    text_layer_type: i64,
    attributes_version: Option<i64>,
    version: Option<i64>,
    additional_attributes: Option<Box<[u8]>>,
    objects: Vec<TextObjectData>,
}

impl TextLayerData {
    /// Owning `Layer.MainId`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Original `LayerType` value.
    #[must_use]
    pub const fn layer_type(&self) -> i64 {
        self.layer_type
    }

    /// Original `TextLayerType` value.
    #[must_use]
    pub const fn text_layer_type(&self) -> i64 {
        self.text_layer_type
    }

    /// Optional `TextLayerAttributesVersion` value.
    #[must_use]
    pub const fn attributes_version(&self) -> Option<i64> {
        self.attributes_version
    }

    /// Optional `TextLayerVersion` value.
    #[must_use]
    pub const fn version(&self) -> Option<i64> {
        self.version
    }

    /// Opaque `TextLayerAddAttributesV01` bytes when present.
    #[must_use]
    pub fn additional_attributes(&self) -> Option<&[u8]> {
        self.additional_attributes.as_deref()
    }

    /// Text objects in their stored order.
    #[must_use]
    pub fn objects(&self) -> &[TextObjectData] {
        &self.objects
    }
}

impl Database {
    /// Reads the text-specific payload for one layer.
    ///
    /// `TextLayerString` is validated as UTF-8. Additional strings and their
    /// opaque attribute records are read from the observed little-endian
    /// length-prefixed array columns. A non-text layer or an older schema
    /// without text columns returns `None`.
    pub fn text_layer(&self, layer_id: i64, limits: Limits) -> Result<Option<TextLayerData>> {
        self.require_column("Layer", "MainId")?;
        if !self.schema().has_column("Layer", "TextLayerType") {
            return Ok(None);
        }
        let text_layer_type = self
            .connection()
            .query_row(
                "SELECT TextLayerType FROM Layer WHERE MainId = ?1 LIMIT 1",
                params![layer_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        let Some(text_layer_type) = text_layer_type else {
            return Ok(None);
        };

        for column in ["LayerType", "TextLayerString", "TextLayerAttributes"] {
            self.require_column("Layer", column)?;
        }
        let optional = |column: &'static str| {
            if self.schema().has_column("Layer", column) {
                column
            } else {
                "NULL"
            }
        };
        let sql = format!(
            "SELECT LayerType, TextLayerString, TextLayerAttributes, \
             {}, {}, {}, {}, {} FROM Layer WHERE MainId = ?1 LIMIT 1",
            optional("TextLayerStringArray"),
            optional("TextLayerAttributesArray"),
            optional("TextLayerAddAttributesV01"),
            optional("TextLayerAttributesVersion"),
            optional("TextLayerVersion"),
        );
        let mut statement = self.connection().prepare(&sql)?;
        let mut rows = statement.query(params![layer_id])?;
        let row = rows.next()?.ok_or_else(|| Error::InvalidDocument {
            reason: format!("text layer {layer_id} disappeared while it was being read"),
        })?;

        let mut total_bytes = 0_u64;
        let first_string = required_bytes(row.get_ref(1)?, 1, "TextLayerString")?;
        account_bytes(
            &mut total_bytes,
            first_string.len(),
            limits.max_text_bytes(),
        )?;
        let first_attributes = required_bytes(row.get_ref(2)?, 2, "TextLayerAttributes")?;
        account_bytes(
            &mut total_bytes,
            first_attributes.len(),
            limits.max_text_bytes(),
        )?;
        if limits.max_text_objects() == 0 {
            return Err(Error::LimitExceeded {
                resource: "text objects per layer",
                value: 1,
                limit: 0,
            });
        }

        let mut strings = vec![decode_text(first_string)?.to_owned()];
        let mut attributes = vec![Box::from(first_attributes)];
        if let Some(array) = optional_bytes(row.get_ref(3)?, 3, "TextLayerStringArray")? {
            account_bytes(&mut total_bytes, array.len(), limits.max_text_bytes())?;
            for item in split_array(array, "TextLayerStringArray", limits.max_text_objects() - 1)? {
                strings.push(decode_text(item)?.to_owned());
            }
        }
        if let Some(array) = optional_bytes(row.get_ref(4)?, 4, "TextLayerAttributesArray")? {
            account_bytes(&mut total_bytes, array.len(), limits.max_text_bytes())?;
            for item in split_array(
                array,
                "TextLayerAttributesArray",
                limits.max_text_objects() - 1,
            )? {
                attributes.push(Box::from(item));
            }
        }
        let additional_attributes =
            optional_bytes(row.get_ref(5)?, 5, "TextLayerAddAttributesV01")?;
        if let Some(bytes) = additional_attributes {
            account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
        }

        let object_count = strings.len() as u64;
        if object_count > limits.max_text_objects() {
            return Err(Error::LimitExceeded {
                resource: "text objects per layer",
                value: object_count,
                limit: limits.max_text_objects(),
            });
        }
        if strings.len() != attributes.len() {
            return Err(Error::InvalidDocument {
                reason: format!(
                    "text layer {layer_id} has {} strings but {} attribute records",
                    strings.len(),
                    attributes.len()
                ),
            });
        }
        let objects = strings
            .into_iter()
            .zip(attributes)
            .map(|(text, attributes)| TextObjectData { text, attributes })
            .collect();
        Ok(Some(TextLayerData {
            layer_id,
            layer_type: row.get(0)?,
            text_layer_type,
            attributes_version: row.get(6)?,
            version: row.get(7)?,
            additional_attributes: additional_attributes.map(Box::from),
            objects,
        }))
    }
}

fn required_bytes<'a>(
    value: ValueRef<'a>,
    column: usize,
    name: &str,
) -> rusqlite::Result<&'a [u8]> {
    optional_bytes(value, column, name)?.ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(column, name.to_owned(), rusqlite::types::Type::Null)
    })
}

fn optional_bytes<'a>(
    value: ValueRef<'a>,
    column: usize,
    name: &str,
) -> rusqlite::Result<Option<&'a [u8]>> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(Some(bytes)),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn account_bytes(total: &mut u64, size: usize, limit: u64) -> Result<()> {
    *total = total
        .checked_add(size as u64)
        .ok_or(Error::OffsetOverflow)?;
    if *total > limit {
        return Err(Error::LimitExceeded {
            resource: "text layer bytes",
            value: *total,
            limit,
        });
    }
    Ok(())
}

fn decode_text(bytes: &[u8]) -> Result<&str> {
    str::from_utf8(bytes).map_err(|_| Error::InvalidDocument {
        reason: "text layer contains invalid UTF-8".to_owned(),
    })
}

fn split_array<'a>(mut bytes: &'a [u8], field: &str, max_items: u64) -> Result<Vec<&'a [u8]>> {
    let mut items = Vec::new();
    while !bytes.is_empty() {
        let next_count = items.len() as u64 + 1;
        if next_count > max_items {
            return Err(Error::LimitExceeded {
                resource: "text objects per layer",
                value: next_count + 1,
                limit: max_items + 1,
            });
        }
        let length_bytes = bytes.get(..4).ok_or_else(|| Error::InvalidDocument {
            reason: format!("{field} ends inside a length prefix"),
        })?;
        let length = u32::from_le_bytes(length_bytes.try_into().expect("four-byte slice")) as usize;
        bytes = &bytes[4..];
        let item = bytes.get(..length).ok_or_else(|| Error::InvalidDocument {
            reason: format!("{field} item extends beyond the stored array"),
        })?;
        items.push(item);
        bytes = &bytes[length..];
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn array(items: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for item in items {
            bytes.extend_from_slice(&(item.len() as u32).to_le_bytes());
            bytes.extend_from_slice(item);
        }
        bytes
    }

    fn text_database(extra_string: &[u8], extra_attributes: &[u8]) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER,
                    LayerType INTEGER,
                    TextLayerType INTEGER,
                    TextLayerString BLOB,
                    TextLayerAttributes BLOB,
                    TextLayerStringArray BLOB,
                    TextLayerAttributesArray BLOB,
                    TextLayerAddAttributesV01 BLOB,
                    TextLayerAttributesVersion INTEGER,
                    TextLayerVersion INTEGER
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Layer VALUES (1, 0, 7, ?1, ?2, ?3, ?4, ?5, 8, 9)",
                params![
                    b"first".as_slice(),
                    b"a1".as_slice(),
                    extra_string,
                    extra_attributes,
                    b"additional".as_slice(),
                ],
            )
            .unwrap();
        connection
            .execute("INSERT INTO Layer (MainId, LayerType) VALUES (2, 1)", [])
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn reads_utf8_text_and_length_prefixed_arrays() {
        let strings = array(&[b"second"]);
        let attributes = array(&[b"a2"]);
        let database = text_database(&strings, &attributes);
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(text.layer_id(), 1);
        assert_eq!(text.layer_type(), 0);
        assert_eq!(text.text_layer_type(), 7);
        assert_eq!(text.attributes_version(), Some(8));
        assert_eq!(text.version(), Some(9));
        assert_eq!(text.additional_attributes(), Some(b"additional".as_slice()));
        assert_eq!(text.objects().len(), 2);
        assert_eq!(text.objects()[0].text(), "first");
        assert_eq!(text.objects()[0].attributes(), b"a1");
        assert_eq!(text.objects()[1].text(), "second");
        assert_eq!(text.objects()[1].attributes(), b"a2");
        assert!(database.text_layer(2, Limits::default()).unwrap().is_none());
        assert!(
            database
                .text_layer(999, Limits::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_malformed_text_and_enforces_limits() {
        let attributes = array(&[b"a2"]);
        let malformed = [4, 0, 0, 0, b'x'];
        let database = text_database(&malformed, &attributes);
        assert!(matches!(
            database.text_layer(1, Limits::default()),
            Err(Error::InvalidDocument { .. })
        ));

        let strings = array(&[b"second"]);
        let database = text_database(&strings, &attributes);
        assert!(matches!(
            database.text_layer(1, Limits::default().with_max_text_objects(1)),
            Err(Error::LimitExceeded { .. })
        ));
        assert!(matches!(
            database.text_layer(1, Limits::default().with_max_text_bytes(1)),
            Err(Error::LimitExceeded { .. })
        ));

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER,
                    LayerType INTEGER,
                    TextLayerType INTEGER,
                    TextLayerString BLOB,
                    TextLayerAttributes BLOB
                 );
                 INSERT INTO Layer VALUES (1, 0, 0, x'FF', x'00');",
            )
            .unwrap();
        let invalid_utf8 = Database::from_connection(connection).unwrap();
        assert!(matches!(
            invalid_utf8.text_layer(1, Limits::default()),
            Err(Error::InvalidDocument { .. })
        ));

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE Layer (MainId INTEGER); INSERT INTO Layer VALUES (1);")
            .unwrap();
        let legacy = Database::from_connection(connection).unwrap();
        assert!(legacy.text_layer(1, Limits::default()).unwrap().is_none());
    }
}
