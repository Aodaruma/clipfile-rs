use std::str;
#[cfg(feature = "write")]
use std::{collections::BTreeSet, ops::Range};

use rusqlite::{OptionalExtension, params, types::ValueRef};

use crate::{Database, Error, Limits, Result};
#[cfg(feature = "write")]
use crate::{DatabaseSchema, EditableDatabase};

/// One text object stored in a CLIP text layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextObjectData {
    text: String,
    attributes: Box<[u8]>,
}

/// Result of adding one text object from a validated attribute template.
#[cfg(feature = "write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextObjectWriteSummary {
    object_index: usize,
    identifier: u32,
}

#[cfg(feature = "write")]
impl TextObjectWriteSummary {
    /// New object index in [`TextLayerData::objects`].
    #[must_use]
    pub const fn object_index(self) -> usize {
        self.object_index
    }

    /// Newly allocated raw text-object identifier from attribute parameter 50.
    #[must_use]
    pub const fn identifier(self) -> u32 {
        self.identifier
    }
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

#[cfg(feature = "write")]
impl EditableDatabase {
    /// Replaces the UTF-8 text of one existing object in a text layer.
    ///
    /// The object's opaque attribute record and every other text object are
    /// preserved byte-for-byte. `object_index` uses the order returned by
    /// [`Database::text_layer`]. Because the paired attribute format can store
    /// text-run offsets, every replacement character must have the same UTF-8
    /// byte width and UTF-16 code-unit width as the corresponding original
    /// character. This method does not synthesize a new text object.
    pub fn replace_text_object_text(
        &self,
        layer_id: i64,
        object_index: usize,
        text: impl AsRef<str>,
        limits: Limits,
    ) -> Result<String> {
        replace_text_object_text(
            self.connection(),
            self.schema(),
            layer_id,
            object_index,
            text.as_ref(),
            limits,
        )
    }

    /// Appends a text object by cloning one existing object's opaque attributes.
    ///
    /// The layer must provide the observed string, attribute, and
    /// `TextLayerAddAttributesV01` array columns. The latter includes the
    /// primary object. Both opaque attribute records are cloned, and their
    /// parameter 50 values are replaced with the same newly allocated
    /// document-wide identifier before all three arrays are updated together.
    ///
    /// To keep every unknown run offset valid, each replacement character must
    /// have the same UTF-8 byte width and UTF-16 code-unit width as the
    /// corresponding template character. Geometry is cloned unchanged, so the
    /// new object may overlap the template. The returned index addresses the
    /// new object in [`Database::text_layer`].
    pub fn add_text_object_from_template(
        &self,
        layer_id: i64,
        template_index: usize,
        text: impl AsRef<str>,
        limits: Limits,
    ) -> Result<TextObjectWriteSummary> {
        add_text_object_from_template(
            self.connection(),
            self.schema(),
            layer_id,
            template_index,
            text.as_ref(),
            limits,
        )
    }

    /// Removes one text object while keeping the layer structurally valid.
    ///
    /// A text layer must retain at least one object. When the primary object
    /// (index zero) is removed, the next object is promoted into the primary
    /// string and attribute columns. The paired additional-attribute record is
    /// removed in the same update. The removed object is returned.
    pub fn remove_text_object(
        &self,
        layer_id: i64,
        object_index: usize,
        limits: Limits,
    ) -> Result<TextObjectData> {
        remove_text_object(
            self.connection(),
            self.schema(),
            layer_id,
            object_index,
            limits,
        )
    }
}

#[cfg(feature = "write")]
fn replace_text_object_text(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    layer_id: i64,
    object_index: usize,
    replacement: &str,
    limits: Limits,
) -> Result<String> {
    for column in [
        "MainId",
        "TextLayerType",
        "TextLayerString",
        "TextLayerAttributes",
    ] {
        if !schema.has_column("Layer", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Layer.{column} is required to edit text"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Layer WHERE MainId = ?1",
        params![layer_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one text layer with ID {layer_id}, found {row_count}"),
        });
    }

    let has_string_array = schema.has_column("Layer", "TextLayerStringArray");
    let has_attributes_array = schema.has_column("Layer", "TextLayerAttributesArray");
    let has_additional_attributes = schema.has_column("Layer", "TextLayerAddAttributesV01");
    let string_array_column = if has_string_array {
        "TextLayerStringArray"
    } else {
        "NULL"
    };
    let attributes_array_column = if has_attributes_array {
        "TextLayerAttributesArray"
    } else {
        "NULL"
    };
    let additional_attributes_column = if has_additional_attributes {
        "TextLayerAddAttributesV01"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT TextLayerType, TextLayerString, TextLayerAttributes, \
         {string_array_column}, {attributes_array_column}, {additional_attributes_column} \
         FROM Layer WHERE MainId = ?1 LIMIT 1"
    );
    let row = connection
        .query_row(&sql, params![layer_id], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                optional_bytes(row.get_ref(1)?, 1, "TextLayerString")?.map(<[u8]>::to_vec),
                optional_bytes(row.get_ref(2)?, 2, "TextLayerAttributes")?.map(<[u8]>::to_vec),
                optional_bytes(row.get_ref(3)?, 3, "TextLayerStringArray")?.map(<[u8]>::to_vec),
                optional_bytes(row.get_ref(4)?, 4, "TextLayerAttributesArray")?.map(<[u8]>::to_vec),
                optional_bytes(row.get_ref(5)?, 5, "TextLayerAddAttributesV01")?
                    .map(<[u8]>::to_vec),
            ))
        })
        .optional()?;
    let Some((
        text_layer_type,
        first,
        first_attributes,
        string_array,
        attributes_array,
        additional_attributes,
    )) = row
    else {
        return Err(Error::InvalidWrite {
            reason: format!("text layer {layer_id} does not exist"),
        });
    };
    if text_layer_type.is_none() {
        return Err(Error::InvalidWrite {
            reason: format!("layer {layer_id} is not a text layer"),
        });
    }
    let first = first.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary string"),
    })?;
    let first_attributes = first_attributes.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary attribute record"),
    })?;

    let mut total_bytes = 0_u64;
    account_bytes(&mut total_bytes, first.len(), limits.max_text_bytes())?;
    account_bytes(
        &mut total_bytes,
        first_attributes.len(),
        limits.max_text_bytes(),
    )?;
    if let Some(bytes) = additional_attributes.as_deref() {
        account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
    }
    let extra_strings = match string_array.as_deref() {
        Some(bytes) => {
            account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
            split_array(
                bytes,
                "TextLayerStringArray",
                limits.max_text_objects().saturating_sub(1),
            )?
        }
        None => Vec::new(),
    };
    let extra_attributes = match attributes_array.as_deref() {
        Some(bytes) => {
            account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
            split_array(
                bytes,
                "TextLayerAttributesArray",
                limits.max_text_objects().saturating_sub(1),
            )?
        }
        None => Vec::new(),
    };
    if extra_strings.len() != extra_attributes.len() {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {} strings but {} attribute records",
                extra_strings.len() + 1,
                extra_attributes.len() + 1
            ),
        });
    }
    let object_count = extra_strings.len().saturating_add(1);
    if object_count as u64 > limits.max_text_objects() {
        return Err(Error::LimitExceeded {
            resource: "text objects per layer",
            value: object_count as u64,
            limit: limits.max_text_objects(),
        });
    }
    if object_index >= object_count {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {object_count} objects, so index {object_index} is invalid"
            ),
        });
    }

    let mut decoded_strings = Vec::with_capacity(object_count);
    decoded_strings.push(decode_text(&first)?);
    for string in &extra_strings {
        decoded_strings.push(decode_text(string)?);
    }
    let replacement_size = replacement.len() as u64;
    if replacement_size > limits.max_text_bytes() {
        return Err(Error::LimitExceeded {
            resource: "replacement text bytes",
            value: replacement_size,
            limit: limits.max_text_bytes(),
        });
    }
    let original = decoded_strings[object_index].to_owned();
    let updated_total = total_bytes
        .checked_sub(original.len() as u64)
        .and_then(|value| value.checked_add(replacement_size))
        .ok_or(Error::OffsetOverflow)?;
    if updated_total > limits.max_text_bytes() {
        return Err(Error::LimitExceeded {
            resource: "text layer bytes after replacement",
            value: updated_total,
            limit: limits.max_text_bytes(),
        });
    }
    if !same_character_encoding_widths(&original, replacement) {
        return Err(Error::InvalidWrite {
            reason:
                "text object replacement must preserve every character's UTF-8 and UTF-16 encoded width"
                    .to_owned(),
        });
    }
    if original == replacement {
        return Ok(original);
    }

    let changed = if object_index == 0 {
        connection.execute(
            "UPDATE Layer SET TextLayerString = ?1 WHERE MainId = ?2",
            params![replacement.as_bytes(), layer_id],
        )?
    } else {
        let mut strings = extra_strings
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        strings[object_index - 1] = replacement.as_bytes().to_vec();
        let encoded = encode_array(&strings)?;
        if encoded.len() as u64 > limits.max_text_bytes() {
            return Err(Error::LimitExceeded {
                resource: "replacement text array bytes",
                value: encoded.len() as u64,
                limit: limits.max_text_bytes(),
            });
        }
        connection.execute(
            "UPDATE Layer SET TextLayerStringArray = ?1 WHERE MainId = ?2",
            params![encoded, layer_id],
        )?
    };
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("text layer {layer_id} is not unique"),
        });
    }
    Ok(original)
}

#[cfg(feature = "write")]
fn add_text_object_from_template(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    layer_id: i64,
    template_index: usize,
    replacement: &str,
    limits: Limits,
) -> Result<TextObjectWriteSummary> {
    for column in [
        "MainId",
        "TextLayerType",
        "TextLayerString",
        "TextLayerAttributes",
        "TextLayerStringArray",
        "TextLayerAttributesArray",
        "TextLayerAddAttributesV01",
    ] {
        if !schema.has_column("Layer", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Layer.{column} is required to add a text object"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Layer WHERE MainId = ?1",
        params![layer_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one text layer with ID {layer_id}, found {row_count}"),
        });
    }

    let sql = "SELECT TextLayerType, TextLayerString, TextLayerAttributes, \
               TextLayerStringArray, TextLayerAttributesArray, TextLayerAddAttributesV01 \
               FROM Layer WHERE MainId = ?1 LIMIT 1";
    let (
        text_layer_type,
        first,
        first_attributes,
        string_array,
        attributes_array,
        additional_attributes,
    ) = connection.query_row(sql, params![layer_id], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            optional_bytes(row.get_ref(1)?, 1, "TextLayerString")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(2)?, 2, "TextLayerAttributes")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(3)?, 3, "TextLayerStringArray")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(4)?, 4, "TextLayerAttributesArray")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(5)?, 5, "TextLayerAddAttributesV01")?.map(<[u8]>::to_vec),
        ))
    })?;
    if text_layer_type.is_none() {
        return Err(Error::InvalidWrite {
            reason: format!("layer {layer_id} is not a text layer"),
        });
    }
    let first = first.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary string"),
    })?;
    let first_attributes = first_attributes.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary attribute record"),
    })?;
    let additional_attributes = additional_attributes.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no TextLayerAddAttributesV01 array"),
    })?;

    let mut total_bytes = 0_u64;
    for bytes in [
        Some(first.as_slice()),
        Some(first_attributes.as_slice()),
        string_array.as_deref(),
        attributes_array.as_deref(),
        Some(additional_attributes.as_slice()),
    ]
    .into_iter()
    .flatten()
    {
        account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
    }
    let extra_strings = match string_array.as_deref() {
        Some(bytes) => split_array(
            bytes,
            "TextLayerStringArray",
            limits.max_text_objects().saturating_sub(1),
        )?,
        None => Vec::new(),
    };
    let extra_attributes = match attributes_array.as_deref() {
        Some(bytes) => split_array(
            bytes,
            "TextLayerAttributesArray",
            limits.max_text_objects().saturating_sub(1),
        )?,
        None => Vec::new(),
    };
    if extra_strings.len() != extra_attributes.len() {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {} strings but {} attribute records",
                extra_strings.len() + 1,
                extra_attributes.len() + 1
            ),
        });
    }
    let object_count = extra_strings.len().saturating_add(1);
    let additional_items = split_array(
        &additional_attributes,
        "TextLayerAddAttributesV01",
        limits.max_text_objects(),
    )?;
    if additional_items.len() != object_count {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {object_count} text objects but {} additional attribute records",
                additional_items.len()
            ),
        });
    }
    let new_count = object_count.checked_add(1).ok_or(Error::OffsetOverflow)?;
    if new_count as u64 > limits.max_text_objects() {
        return Err(Error::LimitExceeded {
            resource: "text objects per layer after addition",
            value: new_count as u64,
            limit: limits.max_text_objects(),
        });
    }
    if template_index >= object_count {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {object_count} objects, so template index {template_index} is invalid"
            ),
        });
    }

    let mut decoded_strings = Vec::with_capacity(object_count);
    decoded_strings.push(decode_text(&first)?);
    for string in &extra_strings {
        decoded_strings.push(decode_text(string)?);
    }
    let template = decoded_strings[template_index];
    if !same_character_encoding_widths(template, replacement) {
        return Err(Error::InvalidWrite {
            reason:
                "new text must preserve every template character's UTF-8 and UTF-16 encoded width"
                    .to_owned(),
        });
    }
    let mut object_attributes = Vec::with_capacity(object_count);
    object_attributes.push(first_attributes.as_slice());
    object_attributes.extend(extra_attributes.iter().copied());
    for (index, (attributes, additional)) in object_attributes
        .iter()
        .copied()
        .zip(additional_items.iter().copied())
        .enumerate()
    {
        let (identifier, _) = text_attribute_identifier(attributes)?;
        let (additional_identifier, _) = text_attribute_identifier(additional)?;
        if identifier != additional_identifier {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "text object {index} has identifier {identifier} but its additional attribute record uses {additional_identifier}"
                ),
            });
        }
    }
    let template_attributes = object_attributes[template_index];
    let template_additional_attributes = additional_items[template_index];
    let identifier = next_text_object_identifier(connection, schema, limits)?;
    let mut cloned_attributes = template_attributes.to_vec();
    replace_text_attribute_identifier(&mut cloned_attributes, identifier)?;
    let mut cloned_additional_attributes = template_additional_attributes.to_vec();
    replace_text_attribute_identifier(&mut cloned_additional_attributes, identifier)?;
    let added_bytes = 12_u64
        .checked_add(replacement.len() as u64)
        .and_then(|value| value.checked_add(cloned_attributes.len() as u64))
        .and_then(|value| value.checked_add(cloned_additional_attributes.len() as u64))
        .ok_or(Error::OffsetOverflow)?;
    let updated_total = total_bytes
        .checked_add(added_bytes)
        .ok_or(Error::OffsetOverflow)?;
    if updated_total > limits.max_text_bytes() {
        return Err(Error::LimitExceeded {
            resource: "text layer bytes after object addition",
            value: updated_total,
            limit: limits.max_text_bytes(),
        });
    }

    let new_strings = append_array_item(string_array.as_deref(), replacement.as_bytes())?;
    let new_attributes = append_array_item(attributes_array.as_deref(), &cloned_attributes)?;
    let new_additional_attributes =
        append_array_item(Some(&additional_attributes), &cloned_additional_attributes)?;
    let parsed_strings = split_array(
        &new_strings,
        "TextLayerStringArray",
        limits.max_text_objects().saturating_sub(1),
    )?;
    let parsed_attributes = split_array(
        &new_attributes,
        "TextLayerAttributesArray",
        limits.max_text_objects().saturating_sub(1),
    )?;
    let parsed_additional_attributes = split_array(
        &new_additional_attributes,
        "TextLayerAddAttributesV01",
        limits.max_text_objects(),
    )?;
    if parsed_strings.len() != object_count
        || parsed_attributes.len() != object_count
        || parsed_additional_attributes.len() != new_count
        || parsed_strings.last().copied() != Some(replacement.as_bytes())
        || parsed_attributes.last().copied() != Some(cloned_attributes.as_slice())
        || parsed_additional_attributes.last().copied()
            != Some(cloned_additional_attributes.as_slice())
    {
        return Err(Error::InvalidWrite {
            reason: "new text object arrays did not round-trip".to_owned(),
        });
    }

    let changed = connection.execute(
        "UPDATE Layer SET TextLayerStringArray = ?1, \
         TextLayerAttributesArray = ?2, TextLayerAddAttributesV01 = ?3 \
         WHERE MainId = ?4",
        params![
            new_strings,
            new_attributes,
            new_additional_attributes,
            layer_id
        ],
    )?;
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("text layer {layer_id} is not unique"),
        });
    }
    Ok(TextObjectWriteSummary {
        object_index: object_count,
        identifier,
    })
}

#[cfg(feature = "write")]
fn remove_text_object(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    layer_id: i64,
    object_index: usize,
    limits: Limits,
) -> Result<TextObjectData> {
    for column in [
        "MainId",
        "TextLayerType",
        "TextLayerString",
        "TextLayerAttributes",
        "TextLayerStringArray",
        "TextLayerAttributesArray",
        "TextLayerAddAttributesV01",
    ] {
        if !schema.has_column("Layer", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Layer.{column} is required to remove a text object"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Layer WHERE MainId = ?1",
        params![layer_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one text layer with ID {layer_id}, found {row_count}"),
        });
    }

    let sql = "SELECT TextLayerType, TextLayerString, TextLayerAttributes, \
               TextLayerStringArray, TextLayerAttributesArray, TextLayerAddAttributesV01 \
               FROM Layer WHERE MainId = ?1 LIMIT 1";
    let (
        text_layer_type,
        primary_string,
        primary_attributes,
        string_array,
        attributes_array,
        additional_attributes,
    ) = connection.query_row(sql, params![layer_id], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            optional_bytes(row.get_ref(1)?, 1, "TextLayerString")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(2)?, 2, "TextLayerAttributes")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(3)?, 3, "TextLayerStringArray")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(4)?, 4, "TextLayerAttributesArray")?.map(<[u8]>::to_vec),
            optional_bytes(row.get_ref(5)?, 5, "TextLayerAddAttributesV01")?.map(<[u8]>::to_vec),
        ))
    })?;
    if text_layer_type.is_none() {
        return Err(Error::InvalidWrite {
            reason: format!("layer {layer_id} is not a text layer"),
        });
    }
    let primary_string = primary_string.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary string"),
    })?;
    let primary_attributes = primary_attributes.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no primary attribute record"),
    })?;
    let additional_attributes = additional_attributes.ok_or_else(|| Error::InvalidWrite {
        reason: format!("text layer {layer_id} has no TextLayerAddAttributesV01 array"),
    })?;

    let mut total_bytes = 0_u64;
    for bytes in [
        Some(primary_string.as_slice()),
        Some(primary_attributes.as_slice()),
        string_array.as_deref(),
        attributes_array.as_deref(),
        Some(additional_attributes.as_slice()),
    ]
    .into_iter()
    .flatten()
    {
        account_bytes(&mut total_bytes, bytes.len(), limits.max_text_bytes())?;
    }

    let mut strings = vec![primary_string];
    if let Some(bytes) = string_array.as_deref() {
        strings.extend(
            split_array(
                bytes,
                "TextLayerStringArray",
                limits.max_text_objects().saturating_sub(1),
            )?
            .into_iter()
            .map(<[u8]>::to_vec),
        );
    }
    let mut attributes = vec![primary_attributes];
    if let Some(bytes) = attributes_array.as_deref() {
        attributes.extend(
            split_array(
                bytes,
                "TextLayerAttributesArray",
                limits.max_text_objects().saturating_sub(1),
            )?
            .into_iter()
            .map(<[u8]>::to_vec),
        );
    }
    if strings.len() != attributes.len() {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {} strings but {} attribute records",
                strings.len(),
                attributes.len()
            ),
        });
    }
    let object_count = strings.len();
    if object_count as u64 > limits.max_text_objects() {
        return Err(Error::LimitExceeded {
            resource: "text objects per layer",
            value: object_count as u64,
            limit: limits.max_text_objects(),
        });
    }
    if object_count <= 1 {
        return Err(Error::InvalidWrite {
            reason: format!("text layer {layer_id} must retain at least one text object"),
        });
    }
    if object_index >= object_count {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {object_count} objects, so object index {object_index} is invalid"
            ),
        });
    }

    let mut additional_items: Vec<Vec<u8>> = split_array(
        &additional_attributes,
        "TextLayerAddAttributesV01",
        limits.max_text_objects(),
    )?
    .into_iter()
    .map(<[u8]>::to_vec)
    .collect();
    if additional_items.len() != object_count {
        return Err(Error::InvalidWrite {
            reason: format!(
                "text layer {layer_id} has {object_count} text objects but {} additional attribute records",
                additional_items.len()
            ),
        });
    }
    for (index, ((string, attributes), additional)) in strings
        .iter()
        .zip(&attributes)
        .zip(&additional_items)
        .enumerate()
    {
        let _ = decode_text(string)?;
        let (identifier, _) = text_attribute_identifier(attributes)?;
        let (additional_identifier, _) = text_attribute_identifier(additional)?;
        if identifier != additional_identifier {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "text object {index} has identifier {identifier} but its additional attribute record uses {additional_identifier}"
                ),
            });
        }
    }

    let removed_string = strings.remove(object_index);
    let removed_attributes = attributes.remove(object_index);
    additional_items.remove(object_index);
    let new_primary_string = strings.remove(0);
    let new_primary_attributes = attributes.remove(0);
    let new_string_array = if strings.is_empty() {
        None
    } else {
        Some(encode_array(&strings)?)
    };
    let new_attributes_array = if attributes.is_empty() {
        None
    } else {
        Some(encode_array(&attributes)?)
    };
    let new_additional_attributes = encode_array(&additional_items)?;

    let changed = connection.execute(
        "UPDATE Layer SET TextLayerString = ?1, TextLayerAttributes = ?2, \
         TextLayerStringArray = ?3, TextLayerAttributesArray = ?4, \
         TextLayerAddAttributesV01 = ?5 WHERE MainId = ?6",
        params![
            new_primary_string,
            new_primary_attributes,
            new_string_array,
            new_attributes_array,
            new_additional_attributes,
            layer_id,
        ],
    )?;
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("text layer {layer_id} is not unique"),
        });
    }

    Ok(TextObjectData {
        text: decode_text(&removed_string)?.to_owned(),
        attributes: removed_attributes.into_boxed_slice(),
    })
}

#[cfg(feature = "write")]
fn same_character_encoding_widths(template: &str, replacement: &str) -> bool {
    template
        .chars()
        .map(|character| (character.len_utf8(), character.len_utf16()))
        .eq(replacement
            .chars()
            .map(|character| (character.len_utf8(), character.len_utf16())))
}

#[cfg(feature = "write")]
fn append_array_item(existing: Option<&[u8]>, item: &[u8]) -> Result<Vec<u8>> {
    let length = u32::try_from(item.len()).map_err(|_| Error::OffsetOverflow)?;
    let existing_len = existing.map_or(0, <[u8]>::len);
    let capacity = existing_len
        .checked_add(4)
        .and_then(|value| value.checked_add(item.len()))
        .ok_or(Error::OffsetOverflow)?;
    let mut bytes = Vec::with_capacity(capacity);
    if let Some(existing) = existing {
        bytes.extend_from_slice(existing);
    }
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(item);
    Ok(bytes)
}

#[cfg(feature = "write")]
fn next_text_object_identifier(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    limits: Limits,
) -> Result<u32> {
    for column in ["TextLayerType", "TextLayerAttributes"] {
        if !schema.has_column("Layer", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Layer.{column} is required to allocate a text object identifier"),
            });
        }
    }
    let attributes_array = if schema.has_column("Layer", "TextLayerAttributesArray") {
        "TextLayerAttributesArray"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT TextLayerAttributes, {attributes_array} \
         FROM Layer WHERE TextLayerType IS NOT NULL"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query([])?;
    let mut identifiers = BTreeSet::new();
    let mut layer_count = 0_u64;
    while let Some(row) = rows.next()? {
        layer_count = layer_count.checked_add(1).ok_or(Error::OffsetOverflow)?;
        if layer_count > limits.max_layers() {
            return Err(Error::LimitExceeded {
                resource: "text layers while allocating an object identifier",
                value: layer_count,
                limit: limits.max_layers(),
            });
        }
        let first = required_bytes(row.get_ref(0)?, 0, "TextLayerAttributes")?;
        if first.len() as u64 > limits.max_text_bytes() {
            return Err(Error::LimitExceeded {
                resource: "text attribute bytes while allocating an object identifier",
                value: first.len() as u64,
                limit: limits.max_text_bytes(),
            });
        }
        let array = optional_bytes(row.get_ref(1)?, 1, "TextLayerAttributesArray")?;
        if array.is_some_and(|bytes| bytes.len() as u64 > limits.max_text_bytes()) {
            return Err(Error::LimitExceeded {
                resource: "text attribute array while allocating an object identifier",
                value: array.map_or(0, |bytes| bytes.len() as u64),
                limit: limits.max_text_bytes(),
            });
        }
        let mut attributes = vec![first];
        if let Some(array) = array {
            attributes.extend(split_array(
                array,
                "TextLayerAttributesArray",
                limits.max_text_objects().saturating_sub(1),
            )?);
        }
        if attributes.len() as u64 > limits.max_text_objects() {
            return Err(Error::LimitExceeded {
                resource: "text objects while allocating an object identifier",
                value: attributes.len() as u64,
                limit: limits.max_text_objects(),
            });
        }
        for attributes in attributes {
            let (identifier, _) = text_attribute_identifier(attributes)?;
            if !identifiers.insert(identifier) {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "text object identifier {identifier} is already used more than once"
                    ),
                });
            }
        }
    }

    if let Some(maximum) = identifiers.last().copied() {
        if let Some(next) = maximum.checked_add(1) {
            return Ok(next);
        }
    }
    let mut candidate = 1_u32;
    for identifier in identifiers {
        if identifier > candidate {
            break;
        }
        if identifier == candidate {
            candidate = candidate
                .checked_add(1)
                .ok_or_else(|| Error::InvalidWrite {
                    reason: "all text object identifiers are occupied".to_owned(),
                })?;
        }
    }
    Ok(candidate)
}

#[cfg(feature = "write")]
fn text_attribute_identifier(attributes: &[u8]) -> Result<(u32, Range<usize>)> {
    let mut offset = 0_usize;
    let mut found = None;
    while offset < attributes.len() {
        let header_end = offset.checked_add(8).ok_or(Error::OffsetOverflow)?;
        let header = attributes
            .get(offset..header_end)
            .ok_or_else(|| Error::InvalidWrite {
                reason: "text attributes end inside a parameter header".to_owned(),
            })?;
        let identifier = u32::from_le_bytes(header[..4].try_into().expect("four bytes"));
        let size = u32::from_le_bytes(header[4..].try_into().expect("four bytes")) as usize;
        let payload_start = header_end;
        let payload_end = payload_start
            .checked_add(size)
            .ok_or(Error::OffsetOverflow)?;
        attributes
            .get(payload_start..payload_end)
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("text attribute parameter {identifier} exceeds its blob"),
            })?;
        if identifier == 50 {
            if size != 4 {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "text object identifier parameter has {size} bytes instead of 4"
                    ),
                });
            }
            if found.is_some() {
                return Err(Error::InvalidWrite {
                    reason: "text attributes repeat object identifier parameter 50".to_owned(),
                });
            }
            let value = u32::from_le_bytes(
                attributes[payload_start..payload_end]
                    .try_into()
                    .expect("four-byte identifier"),
            );
            found = Some((value, payload_start..payload_end));
        }
        offset = payload_end;
    }
    found.ok_or_else(|| Error::InvalidWrite {
        reason: "text attributes have no object identifier parameter 50".to_owned(),
    })
}

#[cfg(feature = "write")]
fn replace_text_attribute_identifier(attributes: &mut [u8], identifier: u32) -> Result<u32> {
    let (original, range) = text_attribute_identifier(attributes)?;
    attributes[range].copy_from_slice(&identifier.to_le_bytes());
    let (round_trip, _) = text_attribute_identifier(attributes)?;
    if round_trip != identifier {
        return Err(Error::InvalidWrite {
            reason: "new text object identifier did not round-trip".to_owned(),
        });
    }
    Ok(original)
}

#[cfg(feature = "write")]
fn encode_array(items: &[Vec<u8>]) -> Result<Vec<u8>> {
    let size = items.iter().try_fold(0_usize, |total, item| {
        let _ = u32::try_from(item.len()).map_err(|_| Error::OffsetOverflow)?;
        total
            .checked_add(4)
            .and_then(|value| value.checked_add(item.len()))
            .ok_or(Error::OffsetOverflow)
    })?;
    let mut bytes = Vec::with_capacity(size);
    for item in items {
        bytes.extend_from_slice(
            &u32::try_from(item.len())
                .map_err(|_| Error::OffsetOverflow)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(item);
    }
    Ok(bytes)
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
    #[cfg(feature = "write")]
    use std::io::Cursor;

    use rusqlite::Connection;
    #[cfg(feature = "write")]
    use rusqlite::MAIN_DB;

    use super::*;

    fn array(items: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for item in items {
            bytes.extend_from_slice(&(item.len() as u32).to_le_bytes());
            bytes.extend_from_slice(item);
        }
        bytes
    }

    #[cfg(feature = "write")]
    fn attributes(identifier: u32, marker: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (kind, value) in [(32_u32, marker), (50, identifier)] {
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&4_u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn text_database(extra_string: &[u8], extra_attributes: &[u8]) -> Database {
        text_database_with_first_attributes(extra_string, extra_attributes, b"a1")
    }

    fn text_database_with_first_attributes(
        extra_string: &[u8],
        extra_attributes: &[u8],
        first_attributes: &[u8],
    ) -> Database {
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
                    first_attributes,
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

    #[cfg(feature = "write")]
    fn writable_text_sample() -> Vec<u8> {
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

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER, LayerType INTEGER, TextLayerType INTEGER,
                    TextLayerString BLOB, TextLayerAttributes BLOB,
                    TextLayerStringArray BLOB, TextLayerAttributesArray BLOB,
                    TextLayerAddAttributesV01 BLOB
                 );
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        let primary_attributes = attributes(1000, 7);
        let additional_attributes = array(&[&primary_attributes]);
        connection
            .execute(
                "INSERT INTO Layer VALUES (1, 0, 0, ?1, ?2, NULL, NULL, ?3)",
                params![
                    b"first".as_slice(),
                    primary_attributes,
                    additional_attributes
                ],
            )
            .unwrap();
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();
        let database_offset = 24 + 16 + 40;
        let mut header = Vec::new();
        push_u64(&mut header, 256);
        push_u64(&mut header, database_offset);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);

        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);
        push_chunk(&mut bytes, b"CHNKHead", &header);
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKSQLi", &database),
            database_offset
        );
        push_chunk(&mut bytes, b"CHNKFoot", b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        bytes
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

    #[cfg(feature = "write")]
    #[test]
    fn replaces_one_text_object_and_preserves_opaque_attributes() {
        let strings = array(&[b"second"]);
        let attributes = array(&[b"a2"]);
        let database = text_database(&strings, &attributes);

        assert_eq!(
            replace_text_object_text(
                database.connection(),
                database.schema(),
                1,
                1,
                "update",
                Limits::default(),
            )
            .unwrap(),
            "second"
        );
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(text.objects()[0].text(), "first");
        assert_eq!(text.objects()[0].attributes(), b"a1");
        assert_eq!(text.objects()[1].text(), "update");
        assert_eq!(text.objects()[1].attributes(), b"a2");
        assert_eq!(text.additional_attributes(), Some(b"additional".as_slice()));

        assert!(matches!(
            replace_text_object_text(
                database.connection(),
                database.schema(),
                1,
                2,
                "missing",
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));

        database
            .connection()
            .execute(
                "UPDATE Layer SET TextLayerString = ?1 WHERE MainId = 1",
                params!["éé".as_bytes()],
            )
            .unwrap();
        assert!(matches!(
            replace_text_object_text(
                database.connection(),
                database.schema(),
                1,
                0,
                "a€",
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(
            database
                .text_layer(1, Limits::default())
                .unwrap()
                .unwrap()
                .objects()[0]
                .text(),
            "éé"
        );

        let limited = text_database(&strings, &attributes);
        assert!(matches!(
            replace_text_object_text(
                limited.connection(),
                limited.schema(),
                1,
                1,
                "update",
                Limits::default().with_max_text_bytes(32),
            ),
            Err(Error::LimitExceeded {
                resource: "text layer bytes",
                value: 33,
                limit: 32,
            })
        ));
        assert_eq!(
            limited
                .text_layer(1, Limits::default())
                .unwrap()
                .unwrap()
                .objects()[1]
                .text(),
            "second"
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn writes_text_through_clip_writer_and_reads_it_back() {
        let mut clip = crate::ClipFile::open(Cursor::new(writable_text_sample())).unwrap();
        let mut writer = clip.writer().unwrap();
        assert_eq!(
            writer
                .database()
                .replace_text_object_text(1, 0, "other", Limits::default())
                .unwrap(),
            "first"
        );
        let added = writer
            .database()
            .add_text_object_from_template(1, 0, "there", Limits::default())
            .unwrap();
        assert_eq!(added.object_index(), 1);
        assert_eq!(added.identifier(), 1001);
        let removed = writer
            .database()
            .remove_text_object(1, 0, Limits::default())
            .unwrap();
        assert_eq!(removed.text(), "other");
        assert_eq!(
            text_attribute_identifier(removed.attributes()).unwrap().0,
            1000
        );
        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();

        let mut rewritten = crate::ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(text.objects().len(), 1);
        assert_eq!(text.objects()[0].text(), "there");
        assert_eq!(
            text_attribute_identifier(text.objects()[0].attributes())
                .unwrap()
                .0,
            1001
        );
        let additional = split_array(
            text.additional_attributes().unwrap(),
            "TextLayerAddAttributesV01",
            Limits::default().max_text_objects(),
        )
        .unwrap();
        assert_eq!(additional.len(), 1);
        assert_eq!(text_attribute_identifier(additional[0]).unwrap().0, 1001);
    }

    #[cfg(feature = "write")]
    #[test]
    fn adds_text_objects_from_primary_and_array_templates_atomically() {
        let strings = array(&[b"second"]);
        let first_attributes = attributes(10, 1);
        let second_attributes = attributes(20, 2);
        let attribute_array = array(&[&second_attributes]);
        let database =
            text_database_with_first_attributes(&strings, &attribute_array, &first_attributes);
        let additional_array = array(&[&first_attributes, &second_attributes]);
        database
            .connection()
            .execute(
                "UPDATE Layer SET TextLayerAddAttributesV01 = ?1 WHERE MainId = 1",
                params![additional_array],
            )
            .unwrap();

        let first = add_text_object_from_template(
            database.connection(),
            database.schema(),
            1,
            0,
            "other",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(first.object_index(), 2);
        assert_eq!(first.identifier(), 21);
        let second = add_text_object_from_template(
            database.connection(),
            database.schema(),
            1,
            1,
            "mirror",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(second.object_index(), 3);
        assert_eq!(second.identifier(), 22);
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(text.objects().len(), 4);
        assert_eq!(text.objects()[2].text(), "other");
        assert_eq!(
            text_attribute_identifier(text.objects()[2].attributes())
                .unwrap()
                .0,
            21
        );
        assert_eq!(text.objects()[3].text(), "mirror");
        assert_eq!(
            text_attribute_identifier(text.objects()[3].attributes())
                .unwrap()
                .0,
            22
        );
        let mut expected_first = first_attributes.clone();
        replace_text_attribute_identifier(&mut expected_first, 21).unwrap();
        assert_eq!(text.objects()[2].attributes(), expected_first);
        let mut expected_second = second_attributes.clone();
        replace_text_attribute_identifier(&mut expected_second, 22).unwrap();
        assert_eq!(text.objects()[3].attributes(), expected_second);
        let additional = split_array(
            text.additional_attributes().unwrap(),
            "TextLayerAddAttributesV01",
            Limits::default().max_text_objects(),
        )
        .unwrap();
        assert_eq!(additional.len(), 4);
        assert_eq!(text_attribute_identifier(additional[0]).unwrap().0, 10);
        assert_eq!(text_attribute_identifier(additional[1]).unwrap().0, 20);
        assert_eq!(text_attribute_identifier(additional[2]).unwrap().0, 21);
        assert_eq!(text_attribute_identifier(additional[3]).unwrap().0, 22);

        let before: (Vec<u8>, Vec<u8>, Vec<u8>) = database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 \
                 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(matches!(
            add_text_object_from_template(
                database.connection(),
                database.schema(),
                1,
                0,
                "a€",
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        let after: (Vec<u8>, Vec<u8>, Vec<u8>) = database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 \
                 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, before);
        assert!(matches!(
            add_text_object_from_template(
                database.connection(),
                database.schema(),
                1,
                0,
                "other",
                Limits::default().with_max_text_objects(4),
            ),
            Err(Error::LimitExceeded { .. })
        ));

        let mismatched_database =
            text_database_with_first_attributes(&strings, &attribute_array, &first_attributes);
        let mismatched_additional = array(&[&attributes(99, 1), &second_attributes]);
        mismatched_database
            .connection()
            .execute(
                "UPDATE Layer SET TextLayerAddAttributesV01 = ?1 WHERE MainId = 1",
                params![mismatched_additional],
            )
            .unwrap();
        let before: (Vec<u8>, Vec<u8>, Vec<u8>) = mismatched_database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(matches!(
            add_text_object_from_template(
                mismatched_database.connection(),
                mismatched_database.schema(),
                1,
                0,
                "other",
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        let after: (Vec<u8>, Vec<u8>, Vec<u8>) = mismatched_database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, before);
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_primary_or_array_text_objects_atomically() {
        fn database_with_three_objects() -> Database {
            let first_attributes = attributes(10, 1);
            let second_attributes = attributes(20, 2);
            let third_attributes = attributes(30, 3);
            let strings = array(&[b"second", b"third"]);
            let attribute_array = array(&[&second_attributes, &third_attributes]);
            let database =
                text_database_with_first_attributes(&strings, &attribute_array, &first_attributes);
            let additional = array(&[&first_attributes, &second_attributes, &third_attributes]);
            database
                .connection()
                .execute(
                    "UPDATE Layer SET TextLayerAddAttributesV01 = ?1 WHERE MainId = 1",
                    params![additional],
                )
                .unwrap();
            database
        }

        let database = database_with_three_objects();
        let removed = remove_text_object(
            database.connection(),
            database.schema(),
            1,
            1,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(removed.text(), "second");
        assert_eq!(
            text_attribute_identifier(removed.attributes()).unwrap().0,
            20
        );
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(
            text.objects()
                .iter()
                .map(TextObjectData::text)
                .collect::<Vec<_>>(),
            ["first", "third"]
        );
        assert_eq!(
            text_attribute_identifier(text.objects()[0].attributes())
                .unwrap()
                .0,
            10
        );
        assert_eq!(
            text_attribute_identifier(text.objects()[1].attributes())
                .unwrap()
                .0,
            30
        );

        let database = database_with_three_objects();
        let removed = remove_text_object(
            database.connection(),
            database.schema(),
            1,
            0,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(removed.text(), "first");
        assert_eq!(
            text_attribute_identifier(removed.attributes()).unwrap().0,
            10
        );
        let text = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(
            text.objects()
                .iter()
                .map(TextObjectData::text)
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        assert_eq!(
            text_attribute_identifier(text.objects()[0].attributes())
                .unwrap()
                .0,
            20
        );
        let additional = split_array(
            text.additional_attributes().unwrap(),
            "TextLayerAddAttributesV01",
            Limits::default().max_text_objects(),
        )
        .unwrap();
        assert_eq!(additional.len(), 2);
        assert_eq!(text_attribute_identifier(additional[0]).unwrap().0, 20);
        assert_eq!(text_attribute_identifier(additional[1]).unwrap().0, 30);

        let before = database.text_layer(1, Limits::default()).unwrap().unwrap();
        assert!(matches!(
            remove_text_object(
                database.connection(),
                database.schema(),
                1,
                2,
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(
            database.text_layer(1, Limits::default()).unwrap().unwrap(),
            before
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn refuses_to_remove_the_only_text_object_or_mismatched_arrays() {
        let bytes = writable_text_sample();
        let mut clip = crate::ClipFile::open(Cursor::new(bytes)).unwrap();
        let writer = clip.writer().unwrap();
        assert!(matches!(
            writer
                .database()
                .remove_text_object(1, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));

        let first_attributes = attributes(10, 1);
        let second_attributes = attributes(20, 2);
        let strings = array(&[b"second"]);
        let attribute_array = array(&[&second_attributes]);
        let database =
            text_database_with_first_attributes(&strings, &attribute_array, &first_attributes);
        let malformed = array(&[&first_attributes]);
        database
            .connection()
            .execute(
                "UPDATE Layer SET TextLayerAddAttributesV01 = ?1 WHERE MainId = 1",
                params![malformed],
            )
            .unwrap();
        let before: (Vec<u8>, Vec<u8>, Vec<u8>) = database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(matches!(
            remove_text_object(
                database.connection(),
                database.schema(),
                1,
                1,
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        let after: (Vec<u8>, Vec<u8>, Vec<u8>) = database
            .connection()
            .query_row(
                "SELECT TextLayerStringArray, TextLayerAttributesArray, \
                 TextLayerAddAttributesV01 FROM Layer WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, before);
    }
}
