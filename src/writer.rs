use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

use rusqlite::{Connection, MAIN_DB, params};

use crate::{
    CHUNK_HEADER_SIZE, ChunkHeader, ChunkKind, ClipFile, Database, DatabaseSchema, Error,
    ExternalBody, ExternalChunkHeader, ExternalObject, ROOT_HEADER_SIZE, Result,
    external::{BlockChecksumPolicy, rebuild_block_data_body_batch},
};

use crate::raster::{OffscreenAttributes, replace_attribute_block_sizes};
#[cfg(feature = "raster")]
use crate::raster::{PixelFormat, RasterEncoder, RasterSource};

const ROOT_MAGIC: &[u8; 8] = b"CSFCHUNK";
const FILE_HEADER_TAG: &[u8; 8] = b"CHNKHead";
const EXTERNAL_TAG: &[u8; 8] = b"CHNKExta";
const SQLITE_TAG: &[u8; 8] = b"CHNKSQLi";
const FOOTER_TAG: &[u8; 8] = b"CHNKFoot";

/// Checksum value to store for a block re-encoded by the writer.
///
/// [`Self::CspCompatible`] generates the checksum used by CLIP STUDIO PAINT.
/// [`Self::Zero`] retains the earlier opt-in compatibility behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BlockChecksumMode {
    /// Generate Adler-32 over the little-endian compressed-size prefix followed
    /// by the zlib-compressed block bytes.
    CspCompatible,
    /// Store zero for the modified block.
    Zero,
}

/// Result of re-encoding one block-data payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockWriteSummary {
    block_index: u32,
    decoded_size: u64,
    original_compressed_size: Option<u64>,
    compressed_size: u64,
    block_record_size: u32,
    original_checksum: u32,
}

impl BlockWriteSummary {
    /// Modified block index.
    #[must_use]
    pub const fn block_index(self) -> u32 {
        self.block_index
    }

    /// Validated decoded byte count.
    #[must_use]
    pub const fn decoded_size(self) -> u64 {
        self.decoded_size
    }

    /// Previous compressed size, or `None` when the block was empty.
    #[must_use]
    pub const fn original_compressed_size(self) -> Option<u64> {
        self.original_compressed_size
    }

    /// New zlib-compressed byte count.
    #[must_use]
    pub const fn compressed_size(self) -> u64 {
        self.compressed_size
    }

    /// Complete serialized size of the rebuilt block record.
    #[must_use]
    pub const fn block_record_size(self) -> u32 {
        self.block_record_size
    }

    /// Opaque checksum value stored before the replacement.
    #[must_use]
    pub const fn original_checksum(self) -> u32 {
        self.original_checksum
    }
}

/// Result of replacing the pixels of one existing raster or layer mask.
#[cfg(feature = "raster")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterWriteSummary {
    layer_id: i64,
    width: u32,
    height: u32,
    format: PixelFormat,
    changed_tiles: u32,
    total_tiles: u32,
}

#[cfg(feature = "raster")]
impl RasterWriteSummary {
    /// Owning layer ID.
    #[must_use]
    pub const fn layer_id(self) -> i64 {
        self.layer_id
    }

    /// Raster width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Raster height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Required row-major pixel format.
    #[must_use]
    pub const fn format(self) -> PixelFormat {
        self.format
    }

    /// Number of tiles whose native bytes changed.
    #[must_use]
    pub const fn changed_tiles(self) -> u32 {
        self.changed_tiles
    }

    /// Total number of tiles in the existing raster.
    #[must_use]
    pub const fn total_tiles(self) -> u32 {
        self.total_tiles
    }
}

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
    added_external_bodies: u64,
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

    /// Total number of external chunks in the rewritten container.
    #[must_use]
    pub const fn external_chunks(self) -> u64 {
        self.external_chunks
    }

    /// Number of external bodies replaced by the caller.
    #[must_use]
    pub const fn replaced_external_bodies(self) -> u64 {
        self.replaced_external_bodies
    }

    /// Number of new external objects added by the caller.
    #[must_use]
    pub const fn added_external_bodies(self) -> u64 {
        self.added_external_bodies
    }
}

/// A validated rewrite session borrowing one source CLIP file.
///
/// The writer currently preserves the observed top-level layout, unknown
/// SQLite columns, and every unchanged external body. Replacements and
/// additions must supply one complete external body unless a more specific
/// semantic editing method is used. Re-encoded blocks can generate
/// CSP-compatible checksums.
pub struct ClipWriter<'source, R> {
    source: &'source mut ClipFile<R>,
    database: EditableDatabase,
    external_replacements: BTreeMap<Vec<u8>, Vec<u8>>,
    external_additions: BTreeMap<Vec<u8>, Vec<u8>>,
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
            external_additions: BTreeMap::new(),
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
        if self.external_additions.contains_key(identifier) {
            return Err(Error::InvalidWrite {
                reason: "a pending external addition cannot also be a source replacement"
                    .to_owned(),
            });
        }
        Ok(self.external_replacements.insert(identifier.to_vec(), body))
    }

    /// Adds one complete opaque body as a new `CHNKExta` object.
    ///
    /// `identifier` must be absent from the source container, the editable
    /// `ExternalChunk` index, pending replacements, and pending additions. The
    /// new chunk is emitted immediately before `CHNKSQLi`; its index row and
    /// absolute offset are generated on a private database clone during
    /// writing. This method does not modify the editable database, so a failed
    /// write leaves both it and the pending addition unchanged.
    pub fn add_external_body(
        &mut self,
        identifier: impl AsRef<[u8]>,
        body: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let identifier = identifier.as_ref();
        if identifier.len() as u64 > self.source.limits().max_identifier_size() {
            return Err(Error::InvalidWrite {
                reason: "new external identifier exceeds the safety limit".to_owned(),
            });
        }
        let body = body.into();
        self.validate_external_body_size_for_update(
            &body,
            self.source.limits().max_write_external_body_size(),
            "new external body size",
        )?;
        let occupied = self.external_addition_occupied_identifiers()?;
        if occupied.contains(identifier) {
            return Err(Error::InvalidWrite {
                reason: "new external identifier is already in use".to_owned(),
            });
        }
        self.validate_external_addition_chunk_count()?;

        self.external_additions.insert(identifier.to_vec(), body);
        Ok(())
    }

    #[cfg(feature = "animation")]
    pub(crate) fn stage_new_external_body(
        &mut self,
        body: Vec<u8>,
        domain_limit: u64,
        resource: &'static str,
    ) -> Result<Vec<u8>> {
        self.validate_external_body_size_for_update(&body, domain_limit, resource)?;
        if 40 > self.source.limits().max_identifier_size() {
            return Err(Error::LimitExceeded {
                resource: "generated external identifier size",
                value: 40,
                limit: self.source.limits().max_identifier_size(),
            });
        }
        self.validate_external_addition_chunk_count()?;
        let mut occupied = self.external_addition_occupied_identifiers()?;
        for _ in 0..128 {
            let mut random: Vec<u8> =
                self.database
                    .connection()
                    .query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
            if random.len() != 16 {
                return Err(Error::InvalidWrite {
                    reason: "SQLite returned an invalid external identifier seed".to_owned(),
                });
            }
            random[6] = (random[6] & 0x0f) | 0x40;
            random[8] = (random[8] & 0x3f) | 0x80;
            let identifier = format_external_identifier(&random);
            if occupied.insert(identifier.clone()) {
                self.external_additions.insert(identifier.clone(), body);
                return Ok(identifier);
            }
        }
        Err(Error::InvalidWrite {
            reason: "could not generate a unique external identifier".to_owned(),
        })
    }

    fn validate_external_addition_chunk_count(&mut self) -> Result<()> {
        let source_count = self.source.chunks().collect::<Result<Vec<_>>>()?.len();
        let source_count = u64::try_from(source_count).map_err(|_| Error::OffsetOverflow)?;
        let pending_count =
            u64::try_from(self.external_additions.len()).map_err(|_| Error::OffsetOverflow)?;
        let resulting_count = source_count
            .checked_add(pending_count)
            .and_then(|count| count.checked_add(1))
            .ok_or(Error::OffsetOverflow)?;
        let limit = self.source.limits().max_top_level_chunks();
        if resulting_count > limit {
            return Err(Error::LimitExceeded {
                resource: "top-level chunks after external addition",
                value: resulting_count,
                limit,
            });
        }
        Ok(())
    }

    fn external_addition_occupied_identifiers(&mut self) -> Result<BTreeSet<Vec<u8>>> {
        let chunks = self.source.chunks().collect::<Result<Vec<_>>>()?;
        let mut occupied = BTreeSet::new();
        for chunk in &chunks {
            if chunk.kind() != ChunkKind::External {
                continue;
            }
            let header = self.source.external_chunk_header(chunk)?;
            if !occupied.insert(header.identifier().to_vec()) {
                return Err(Error::InvalidWrite {
                    reason: "source contains duplicate external identifiers".to_owned(),
                });
            }
        }
        let mut indexed = BTreeSet::new();
        for record in self.database.database.external_chunks()? {
            if !indexed.insert(record.identifier().to_vec()) {
                return Err(Error::InvalidWrite {
                    reason: "editable ExternalChunk index contains duplicate identifiers"
                        .to_owned(),
                });
            }
        }
        occupied.extend(indexed);
        occupied.extend(self.external_replacements.keys().cloned());
        occupied.extend(self.external_additions.keys().cloned());
        Ok(occupied)
    }

    pub(crate) fn external_body_for_update(
        &mut self,
        identifier: &[u8],
        domain_limit: u64,
    ) -> Result<Vec<u8>> {
        if identifier.len() as u64 > self.source.limits().max_identifier_size() {
            return Err(Error::InvalidWrite {
                reason: "replacement external identifier exceeds the safety limit".to_owned(),
            });
        }
        let limit = domain_limit.min(self.source.limits().max_write_external_body_size());
        if let Some(body) = self.external_additions.get(identifier) {
            self.validate_external_body_size_for_update(
                body,
                domain_limit,
                "new external body for semantic update",
            )?;
            return Ok(body.clone());
        }
        if let Some(body) = self.external_replacements.get(identifier) {
            self.validate_external_body_size_for_update(
                body,
                domain_limit,
                "external body for semantic update",
            )?;
            return Ok(body.clone());
        }
        let object = self.source_external_object(identifier)?;
        self.source.read_external_body(&object, limit)
    }

    #[cfg(feature = "animation")]
    pub(crate) fn replace_or_update_external_body(
        &mut self,
        identifier: &[u8],
        body: Vec<u8>,
    ) -> Result<Option<Vec<u8>>> {
        let limit = self.source.limits().max_write_external_body_size();
        self.validate_external_body_size_for_update(
            &body,
            limit,
            "external body for semantic update",
        )?;
        if let Some(pending) = self.external_additions.get_mut(identifier) {
            return Ok(Some(std::mem::replace(pending, body)));
        }
        self.replace_external_body(identifier, body)
    }

    pub(crate) fn validate_external_body_size_for_update(
        &self,
        body: &[u8],
        domain_limit: u64,
        resource: &'static str,
    ) -> Result<()> {
        let limit = domain_limit.min(self.source.limits().max_write_external_body_size());
        if body.len() as u64 > limit {
            return Err(Error::LimitExceeded {
                resource,
                value: body.len() as u64,
                limit,
            });
        }
        Ok(())
    }

    /// Re-encodes one block from validated native decoded bytes.
    ///
    /// `decoded` must have exactly `channels × width × height` bytes according
    /// to the existing block parameters. The writer preserves every other
    /// block, status, checksum, index, and parameter record, zlib-compresses
    /// this block, and replaces the complete external body.
    ///
    /// A previously empty block may be populated. Repeated calls for the same
    /// external object build on the pending replacement. Callers explicitly
    /// choose CSP-compatible checksum generation or the legacy zero mode.
    pub fn replace_block_bytes(
        &mut self,
        identifier: impl AsRef<[u8]>,
        block_index: u32,
        decoded: impl AsRef<[u8]>,
        checksum_mode: BlockChecksumMode,
    ) -> Result<BlockWriteSummary> {
        let mut replacements = BTreeMap::new();
        replacements.insert(block_index, decoded.as_ref().to_vec());
        self.replace_block_bytes_batch(identifier.as_ref(), &replacements, checksum_mode)?
            .pop()
            .ok_or_else(|| Error::InvalidWrite {
                reason: "single block replacement returned no summary".to_owned(),
            })
    }

    fn replace_block_bytes_batch(
        &mut self,
        identifier: &[u8],
        replacements: &BTreeMap<u32, Vec<u8>>,
        checksum_mode: BlockChecksumMode,
    ) -> Result<Vec<BlockWriteSummary>> {
        if identifier.len() as u64 > self.source.limits().max_identifier_size() {
            return Err(Error::InvalidWrite {
                reason: "replacement external identifier exceeds the safety limit".to_owned(),
            });
        }
        let object = self.source_external_object(identifier)?;
        if object.body() != ExternalBody::BlockData {
            return Err(Error::InvalidWrite {
                reason: "replacement external object is not block data".to_owned(),
            });
        }

        let pending = self.external_replacements.remove(identifier);
        let had_pending = pending.is_some();
        let body = match pending {
            Some(body) => body,
            None => {
                let limit = self.source.limits().max_write_external_body_size();
                self.source.read_external_body(&object, limit)?
            }
        };
        let checksum_policy = match checksum_mode {
            BlockChecksumMode::CspCompatible => BlockChecksumPolicy::CspCompatible,
            BlockChecksumMode::Zero => BlockChecksumPolicy::Zero,
        };
        let limits = self.source.limits();
        let rebuilt = rebuild_block_data_body_batch(
            &body,
            replacements,
            checksum_policy,
            limits.max_blocks_per_external(),
            limits.max_decompressed_block_size(),
            limits.max_write_external_body_size(),
        );
        let rebuilt = match rebuilt {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                if had_pending {
                    self.external_replacements.insert(identifier.to_vec(), body);
                }
                return Err(error);
            }
        };
        if let Err(error) = self.update_offscreen_block_sizes(
            identifier,
            &rebuilt
                .blocks
                .iter()
                .map(|block| (block.block_index, block.block_record_size))
                .collect::<Vec<_>>(),
        ) {
            if had_pending {
                self.external_replacements.insert(identifier.to_vec(), body);
            }
            return Err(error);
        }
        self.external_replacements
            .insert(identifier.to_vec(), rebuilt.body);
        Ok(rebuilt
            .blocks
            .into_iter()
            .map(|block| BlockWriteSummary {
                block_index: block.block_index,
                decoded_size: block.decoded_size,
                original_compressed_size: block.original_compressed_size,
                compressed_size: block.compressed_size,
                block_record_size: block.block_record_size,
                original_checksum: block.original_checksum,
            })
            .collect())
    }

    /// Replaces the complete render raster of an existing layer.
    ///
    /// `pixels` must contain the existing raster dimensions in row-major
    /// [`PixelFormat::Rgba8`] or [`PixelFormat::Gray8`] form. Only tiles whose
    /// semantic pixels changed are re-encoded; padding outside the bitmap is
    /// preserved. This method cannot add a missing external object.
    #[cfg(feature = "raster")]
    pub fn replace_layer_raster_pixels(
        &mut self,
        layer_id: i64,
        format: PixelFormat,
        pixels: impl AsRef<[u8]>,
        checksum_mode: BlockChecksumMode,
    ) -> Result<RasterWriteSummary> {
        let source = self
            .database
            .database
            .layer_raster_source(layer_id)?
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("layer {layer_id} has no render raster"),
            })?;
        self.replace_raster_pixels(layer_id, source, format, pixels.as_ref(), checksum_mode)
    }

    /// Replaces the complete mask raster of an existing layer.
    ///
    /// The input and preservation rules are the same as
    /// [`Self::replace_layer_raster_pixels`].
    #[cfg(feature = "raster")]
    pub fn replace_layer_mask_pixels(
        &mut self,
        layer_id: i64,
        format: PixelFormat,
        pixels: impl AsRef<[u8]>,
        checksum_mode: BlockChecksumMode,
    ) -> Result<RasterWriteSummary> {
        let source = self
            .database
            .database
            .layer_mask_raster_source(layer_id)?
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("layer {layer_id} has no layer-mask raster"),
            })?;
        self.replace_raster_pixels(layer_id, source, format, pixels.as_ref(), checksum_mode)
    }

    #[cfg(feature = "raster")]
    fn replace_raster_pixels(
        &mut self,
        layer_id: i64,
        source: RasterSource,
        format: PixelFormat,
        pixels: &[u8],
        checksum_mode: BlockChecksumMode,
    ) -> Result<RasterWriteSummary> {
        let identifier = source
            .external_identifier()
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("layer {layer_id} raster has no external block-data identifier"),
            })?
            .to_vec();
        if self.external_replacements.contains_key(&identifier) {
            return Err(Error::InvalidWrite {
                reason: "raster replacement cannot follow a pending replacement for the same external object"
                    .to_owned(),
            });
        }
        self.validate_raster_alias_layout(&identifier, source.attributes())?;
        let object = self.source_external_object(&identifier)?;
        if object.body() != ExternalBody::BlockData {
            return Err(Error::InvalidWrite {
                reason: "raster external object is not block data".to_owned(),
            });
        }
        let block_data = self.source.read_block_data(&object)?;
        let mut blocks = BTreeMap::new();
        for block in block_data.blocks() {
            if blocks.insert(block.index(), block.clone()).is_some() {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "raster external object contains duplicate block index {}",
                        block.index()
                    ),
                });
            }
        }
        let limits = self.source.limits();
        let encoder = RasterEncoder::new(
            source.attributes(),
            format,
            pixels,
            limits.max_canvas_dimension(),
            limits.max_raster_bytes(),
            limits.max_decompressed_block_size(),
        )?;
        if blocks.len() as u64 != u64::from(encoder.tile_count()) {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "raster has {} block records, expected {}",
                    blocks.len(),
                    encoder.tile_count()
                ),
            });
        }

        let mut replacements = BTreeMap::new();
        for tile_index in 0..encoder.tile_count() {
            let block = blocks.get(&tile_index).ok_or_else(|| Error::InvalidWrite {
                reason: format!("raster has no block index {tile_index}"),
            })?;
            let original = self
                .source
                .decode_tile(block)?
                .map_or_else(|| encoder.default_tile(), |tile| tile.into_bytes());
            let encoded = encoder.encode_tile(tile_index, Some(original.clone()))?;
            if encoded != original {
                replacements.insert(tile_index, encoded);
            }
        }
        let changed_tiles = u32::try_from(replacements.len()).map_err(|_| Error::OffsetOverflow)?;
        if !replacements.is_empty() {
            self.replace_block_bytes_batch(&identifier, &replacements, checksum_mode)?;
        }
        Ok(RasterWriteSummary {
            layer_id,
            width: source.attributes().bitmap_width(),
            height: source.attributes().bitmap_height(),
            format,
            changed_tiles,
            total_tiles: encoder.tile_count(),
        })
    }

    #[cfg(feature = "raster")]
    fn validate_raster_alias_layout(
        &self,
        identifier: &[u8],
        expected: &OffscreenAttributes,
    ) -> Result<()> {
        let rows = self.offscreen_attribute_rows(identifier)?;
        if rows.is_empty() {
            return Err(Error::InvalidWrite {
                reason: "raster external identifier has no matching Offscreen row".to_owned(),
            });
        }
        for (offscreen_id, attributes) in rows {
            let parsed = OffscreenAttributes::parse(&attributes)?;
            if parsed != *expected {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "Offscreen {offscreen_id} shares the raster external identifier but has a different semantic layout"
                    ),
                });
            }
        }
        Ok(())
    }

    fn offscreen_attribute_rows(&self, identifier: &[u8]) -> Result<Vec<(i64, Vec<u8>)>> {
        let schema = self.database.schema();
        if !["MainId", "Attribute", "BlockData"]
            .into_iter()
            .all(|column| schema.has_column("Offscreen", column))
        {
            return Ok(Vec::new());
        }
        let mut statement = self.database.connection().prepare(
            "SELECT MainId, Attribute FROM Offscreen \
                 WHERE CAST(BlockData AS BLOB) = ?1",
        )?;
        let rows = statement.query_map(params![identifier], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn update_offscreen_block_sizes(
        &mut self,
        identifier: &[u8],
        replacements: &[(u32, u32)],
    ) -> Result<()> {
        let rows = self.offscreen_attribute_rows(identifier)?;
        let mut attributes_replacements = Vec::with_capacity(rows.len());
        for (offscreen_id, attributes) in rows {
            let parsed = OffscreenAttributes::parse(&attributes)?;
            let mut block_sizes = parsed.block_sizes().to_vec();
            for &(block_index, block_record_size) in replacements {
                let slot = block_sizes
                    .get_mut(usize::try_from(block_index).map_err(|_| Error::OffsetOverflow)?)
                    .ok_or_else(|| Error::InvalidWrite {
                        reason: format!(
                            "Offscreen {offscreen_id} has no BlockSize entry {block_index}"
                        ),
                    })?;
                *slot = block_record_size;
            }
            attributes_replacements.push((
                offscreen_id,
                replace_attribute_block_sizes(&attributes, &block_sizes)?,
            ));
        }
        self.restore_offscreen_attribute_rows(&attributes_replacements)
    }

    fn restore_offscreen_attribute_rows(&mut self, rows: &[(i64, Vec<u8>)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let transaction = self.database.connection_mut().transaction()?;
        for (offscreen_id, attributes) in rows {
            let updated = transaction.execute(
                "UPDATE Offscreen SET Attribute = ?1 WHERE MainId = ?2",
                params![attributes, offscreen_id],
            )?;
            if updated != 1 {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "Offscreen {offscreen_id} attribute update affected {updated} rows"
                    ),
                });
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Removes a pending external-body replacement.
    pub fn remove_external_replacement(&mut self, identifier: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        self.external_replacements.remove(identifier.as_ref())
    }

    /// Removes and returns one pending new external body.
    pub fn remove_external_addition(&mut self, identifier: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        self.unstage_new_external_body(identifier.as_ref())
    }

    pub(crate) fn unstage_new_external_body(&mut self, identifier: &[u8]) -> Option<Vec<u8>> {
        self.external_additions.remove(identifier)
    }

    /// Number of pending external-body replacements.
    #[must_use]
    pub fn replacement_count(&self) -> usize {
        self.external_replacements.len()
    }

    /// Number of pending new external objects.
    #[must_use]
    pub fn addition_count(&self) -> usize {
        self.external_additions.len()
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
        for (identifier, body) in &self.external_additions {
            if identifiers.contains(identifier)
                || self.external_replacements.contains_key(identifier)
            {
                return Err(Error::InvalidWrite {
                    reason: "new external identifier conflicts with a source object or replacement"
                        .to_owned(),
                });
            }
            if identifier.len() as u64 > self.source.limits().max_identifier_size() {
                return Err(Error::InvalidWrite {
                    reason: "new external identifier exceeds the safety limit".to_owned(),
                });
            }
            if body.len() as u64 > self.source.limits().max_write_external_body_size() {
                return Err(Error::LimitExceeded {
                    resource: "new external body size",
                    value: body.len() as u64,
                    limit: self.source.limits().max_write_external_body_size(),
                });
            }
            identifiers.insert(identifier.clone());
        }
        let output_chunk_count = u64::try_from(chunks.len())
            .map_err(|_| Error::OffsetOverflow)?
            .checked_add(
                u64::try_from(self.external_additions.len()).map_err(|_| Error::OffsetOverflow)?,
            )
            .ok_or(Error::OffsetOverflow)?;
        if output_chunk_count > self.source.limits().max_top_level_chunks() {
            return Err(Error::LimitExceeded {
                resource: "top-level chunks after external additions",
                value: output_chunk_count,
                limit: self.source.limits().max_top_level_chunks(),
            });
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
                    for (identifier, body) in &self.external_additions {
                        if external_offsets
                            .insert(identifier.clone(), output_offset)
                            .is_some()
                        {
                            return Err(Error::InvalidWrite {
                                reason: "new external identifier is not unique".to_owned(),
                            });
                        }
                        output_offset = next_chunk_offset(
                            output_offset,
                            external_payload_size(identifier, body)?,
                        )?;
                    }
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
        insert_external_index_rows(&working, &self.external_additions, &external_offsets)?;
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
            if chunk.kind() == ChunkKind::Sqlite {
                for (identifier, body) in &self.external_additions {
                    output_file_size = next_chunk_offset(
                        output_file_size,
                        external_payload_size(identifier, body)?,
                    )?;
                }
            }
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
            external_chunks: u64::try_from(external_headers.len())
                .map_err(|_| Error::OffsetOverflow)?
                .checked_add(
                    u64::try_from(self.external_additions.len())
                        .map_err(|_| Error::OffsetOverflow)?,
                )
                .ok_or(Error::OffsetOverflow)?,
            replaced_external_bodies: self.external_replacements.len() as u64,
            added_external_bodies: self.external_additions.len() as u64,
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
                    for (identifier, body) in &self.external_additions {
                        write_external_chunk(destination, identifier, body)?;
                    }
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

    pub(crate) fn source_external_object(&mut self, identifier: &[u8]) -> Result<ExternalObject> {
        let chunks = self.source.chunks().collect::<Result<Vec<_>>>()?;
        let mut found = None;
        for chunk in chunks {
            if chunk.kind() != ChunkKind::External {
                continue;
            }
            let object = self.source.inspect_external_chunk(&chunk)?;
            if object.header().identifier() != identifier {
                continue;
            }
            if found.is_some() {
                return Err(Error::InvalidWrite {
                    reason: "source contains duplicate external identifiers".to_owned(),
                });
            }
            found = Some(object);
        }
        found.ok_or_else(|| Error::InvalidWrite {
            reason: "replacement identifier does not exist in the source".to_owned(),
        })
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

fn insert_external_index_rows(
    connection: &Connection,
    additions: &BTreeMap<Vec<u8>, Vec<u8>>,
    external_offsets: &BTreeMap<Vec<u8>, u64>,
) -> Result<()> {
    for identifier in additions.keys() {
        let existing: i64 = connection.query_row(
            "SELECT count(*) FROM ExternalChunk \
             WHERE CAST(ExternalID AS BLOB) = ?1",
            params![identifier],
            |row| row.get(0),
        )?;
        if existing != 0 {
            return Err(Error::InvalidWrite {
                reason: "new external identifier already exists in the editable index".to_owned(),
            });
        }
        let offset =
            external_offsets
                .get(identifier)
                .copied()
                .ok_or_else(|| Error::InvalidWrite {
                    reason: "new external object has no calculated output offset".to_owned(),
                })?;
        let offset = i64::try_from(offset).map_err(|_| Error::OffsetOverflow)?;
        let inserted = connection.execute(
            "INSERT INTO ExternalChunk (ExternalID, Offset) VALUES (?1, ?2)",
            params![identifier, offset],
        )?;
        if inserted != 1 {
            return Err(Error::InvalidWrite {
                reason: "new ExternalChunk index row was not inserted exactly once".to_owned(),
            });
        }
    }
    Ok(())
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

#[cfg(feature = "animation")]
fn format_external_identifier(uuid: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8; 8] = b"extrnlid";
    const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

    debug_assert_eq!(uuid.len(), 16);
    let mut identifier = Vec::with_capacity(40);
    identifier.extend_from_slice(PREFIX);
    for byte in uuid {
        identifier.push(UPPER_HEX[usize::from(byte >> 4)]);
        identifier.push(UPPER_HEX[usize::from(byte & 0x0f)]);
    }
    identifier
}

fn next_chunk_offset(offset: u64, payload_size: u64) -> Result<u64> {
    offset
        .checked_add(CHUNK_HEADER_SIZE)
        .and_then(|value| value.checked_add(payload_size))
        .ok_or(Error::OffsetOverflow)
}

fn external_payload_size(identifier: &[u8], body: &[u8]) -> Result<u64> {
    16_u64
        .checked_add(u64::try_from(identifier.len()).map_err(|_| Error::OffsetOverflow)?)
        .and_then(|size| size.checked_add(body.len() as u64))
        .ok_or(Error::OffsetOverflow)
}

fn write_external_chunk<W: Write>(
    destination: &mut W,
    identifier: &[u8],
    body: &[u8],
) -> Result<()> {
    write_chunk_header(
        destination,
        EXTERNAL_TAG,
        external_payload_size(identifier, body)?,
    )?;
    destination.write_all(&(identifier.len() as u64).to_be_bytes())?;
    destination.write_all(identifier)?;
    destination.write_all(&(body.len() as u64).to_be_bytes())?;
    destination.write_all(body)?;
    Ok(())
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
        io::{Cursor, Read},
        time::{SystemTime, UNIX_EPOCH},
    };

    use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

    use super::*;

    const EXTERNAL_ID: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";
    const NEW_EXTERNAL_ID: &[u8] = b"extrnlid00112233445546778899AABBCCDDEEFF";
    const BLOCK_BEGIN_TEST: &[u8] = b"\0B\0l\0o\0c\0k\0D\0a\0t\0a\0B\0e\0g\0i\0n\0C\0h\0u\0n\0k";
    const BLOCK_END_TEST: &[u8] = b"\0B\0l\0o\0c\0k\0D\0a\0t\0a\0E\0n\0d\0C\0h\0u\0n\0k";
    const BLOCK_STATUS_TEST: &[u8] = b"\0B\0l\0o\0c\0k\0S\0t\0a\0t\0u\0s";
    const BLOCK_CHECKSUM_TEST: &[u8] = b"\0B\0l\0o\0c\0k\0C\0h\0e\0c\0k\0S\0u\0m";

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u32_be(bytes: &mut Vec<u8>, value: u32) {
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
        sample_with_external_body(b"abc")
    }

    fn sample_with_external_body(body: &[u8]) -> Vec<u8> {
        sample_with_external_body_and_offscreen(body, &[])
    }

    fn sample_with_external_body_and_offscreen(
        body: &[u8],
        offscreen_rows: &[(i64, Vec<u8>)],
    ) -> Vec<u8> {
        let header_payload_size = 40_u64;
        let external_offset = ROOT_HEADER_SIZE + CHUNK_HEADER_SIZE + header_payload_size;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ExternalChunk (ExternalID BLOB NOT NULL, Offset INTEGER NOT NULL);
                 CREATE TABLE Offscreen (
                    MainId INTEGER,
                    LayerId INTEGER,
                    Attribute BLOB NOT NULL,
                    BlockData BLOB NOT NULL
                 );
                 CREATE TABLE Layer (MainId INTEGER, LayerRenderMipmap INTEGER);
                 CREATE TABLE Mipmap (MainId INTEGER, BaseMipmapInfo INTEGER);
                 CREATE TABLE MipmapInfo (MainId INTEGER, Offscreen INTEGER);
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
        for (main_id, attribute) in offscreen_rows {
            connection
                .execute(
                    "INSERT INTO Offscreen VALUES (?1, 20, ?2, ?3)",
                    params![main_id, attribute, EXTERNAL_ID],
                )
                .unwrap();
        }
        if let Some((offscreen_id, _)) = offscreen_rows.first() {
            connection
                .execute_batch(
                    "INSERT INTO Layer VALUES (20, 30);
                     INSERT INTO Mipmap VALUES (30, 40);",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO MipmapInfo VALUES (40, ?1)",
                    params![offscreen_id],
                )
                .unwrap();
        }
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();

        let mut external = Vec::new();
        push_u64(&mut external, EXTERNAL_ID.len() as u64);
        external.extend_from_slice(EXTERNAL_ID);
        push_u64(&mut external, body.len() as u64);
        external.extend_from_slice(body);
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

    fn test_offscreen_attribute(block_record_size: u32) -> Vec<u8> {
        test_offscreen_attribute_with_layout(block_record_size, 2, 2, [0; 16])
    }

    #[cfg(feature = "raster")]
    fn test_gray_offscreen_attribute(
        block_record_size: u32,
        bitmap_width: u32,
        bitmap_height: u32,
    ) -> Vec<u8> {
        let mut packing = [0_u32; 16];
        packing[1] = 1;
        packing[3] = 1;
        packing[8] = 8 << 5;
        packing[10] = 2;
        packing[11] = 2;
        test_offscreen_attribute_with_layout(
            block_record_size,
            bitmap_width,
            bitmap_height,
            packing,
        )
    }

    fn test_offscreen_attribute_with_layout(
        block_record_size: u32,
        bitmap_width: u32,
        bitmap_height: u32,
        packing: [u32; 16],
    ) -> Vec<u8> {
        fn push_label(bytes: &mut Vec<u8>, value: &str) {
            push_u32_be(bytes, value.encode_utf16().count() as u32);
            for character in value.encode_utf16() {
                bytes.extend_from_slice(&character.to_be_bytes());
            }
        }

        let mut parameter = Vec::new();
        push_label(&mut parameter, "Parameter");
        for value in [bitmap_width, bitmap_height, 1, 1] {
            push_u32_be(&mut parameter, value);
        }
        for value in packing {
            push_u32_be(&mut parameter, value);
        }

        let mut init = Vec::new();
        push_label(&mut init, "InitColor");
        for value in [20, 0, 0, 0, 4] {
            push_u32_be(&mut init, value);
        }

        let mut blocks = Vec::new();
        push_label(&mut blocks, "BlockSize");
        for value in [12, 1, 4, block_record_size] {
            push_u32_be(&mut blocks, value);
        }

        let mut bytes = Vec::new();
        for value in [
            16,
            parameter.len() as u32,
            init.len() as u32,
            blocks.len() as u32,
        ] {
            push_u32_be(&mut bytes, value);
        }
        bytes.extend(parameter);
        bytes.extend(init);
        bytes.extend(blocks);
        bytes
    }

    fn test_block_body(decoded: Option<&[u8]>, checksum: u32) -> Vec<u8> {
        let compressed = decoded.map(|decoded| {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(decoded).unwrap();
            encoder.finish().unwrap()
        });

        let mut body = Vec::new();
        let block_start = body.len();
        push_u32_be(&mut body, 0);
        push_u32_be(&mut body, 19);
        body.extend_from_slice(BLOCK_BEGIN_TEST);
        push_u32_be(&mut body, 0);
        body.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 2, 0, 0, 0, 2]);
        if let Some(compressed) = &compressed {
            push_u32_be(&mut body, 1);
            push_u32_be(&mut body, compressed.len() as u32 + 4);
            body.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            body.extend_from_slice(compressed);
        } else {
            push_u32_be(&mut body, 0);
        }
        push_u32_be(&mut body, 17);
        body.extend_from_slice(BLOCK_END_TEST);
        let block_size = (body.len() - block_start) as u32;
        body[block_start..block_start + 4].copy_from_slice(&block_size.to_be_bytes());

        for (marker, value) in [(BLOCK_STATUS_TEST, 1_u32), (BLOCK_CHECKSUM_TEST, checksum)] {
            push_u32_be(&mut body, (marker.len() / 2) as u32);
            body.extend_from_slice(marker);
            push_u32_be(&mut body, 12);
            push_u32_be(&mut body, 1);
            push_u32_be(&mut body, 4);
            push_u32_be(&mut body, value);
        }
        body
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
    fn adds_an_external_object_before_sqlite_and_indexes_its_offset() {
        let path = temp_output_path();
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let original_database_offset = clip.file_header().database_offset();
        let mut writer = clip.writer().unwrap();
        writer
            .add_external_body(NEW_EXTERNAL_ID, b"new external body".to_vec())
            .unwrap();
        assert_eq!(writer.addition_count(), 1);

        let summary = writer.write_to_path(&path).unwrap();
        assert_eq!(summary.external_chunks(), 2);
        assert_eq!(summary.replaced_external_bodies(), 0);
        assert_eq!(summary.added_external_bodies(), 1);
        assert!(summary.database_offset() > original_database_offset);
        let editable_rows: i64 = writer
            .database()
            .connection()
            .query_row("SELECT count(*) FROM ExternalChunk", [], |row| row.get(0))
            .unwrap();
        assert_eq!(editable_rows, 1);

        let mut rewritten = ClipFile::open(File::open(&path).unwrap()).unwrap();
        rewritten.validate().unwrap();
        let chunks = rewritten.chunks().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(
            chunks.iter().map(ChunkHeader::kind).collect::<Vec<_>>(),
            [
                ChunkKind::Header,
                ChunkKind::External,
                ChunkKind::External,
                ChunkKind::Sqlite,
                ChunkKind::Footer,
            ]
        );
        let new_chunk = &chunks[2];
        assert_eq!(
            new_chunk.offset() + CHUNK_HEADER_SIZE + new_chunk.payload_size(),
            summary.database_offset()
        );
        let object = rewritten.inspect_external_chunk(new_chunk).unwrap();
        assert_eq!(object.header().identifier(), NEW_EXTERNAL_ID);
        assert_eq!(
            rewritten.read_external_body(&object, 1024).unwrap(),
            b"new external body"
        );
        let database = rewritten.open_database().unwrap();
        let record = database.external_chunk(NEW_EXTERNAL_ID).unwrap().unwrap();
        assert_eq!(record.offset(), new_chunk.offset());
        rewritten.validate_external_index(&database).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[cfg(feature = "animation")]
    #[test]
    fn generates_format_compatible_external_ids_and_can_roll_them_back() {
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        let identifier = writer
            .stage_new_external_body(b"animation mixer".to_vec(), 1024, "test animation body")
            .unwrap();
        assert_eq!(identifier.len(), 40);
        assert_eq!(&identifier[..8], b"extrnlid");
        assert!(identifier[8..].iter().all(u8::is_ascii_hexdigit));
        assert!(
            identifier[8..]
                .iter()
                .all(|byte| !byte.is_ascii_lowercase())
        );
        assert_eq!(identifier[20], b'4');
        assert!(matches!(identifier[24], b'8' | b'9' | b'A' | b'B'));
        assert_eq!(writer.addition_count(), 1);
        assert_eq!(
            writer.external_body_for_update(&identifier, 1024).unwrap(),
            b"animation mixer"
        );
        let previous = writer
            .replace_or_update_external_body(&identifier, b"updated mixer".to_vec())
            .unwrap()
            .unwrap();
        assert_eq!(previous, b"animation mixer");
        assert_eq!(
            writer.external_body_for_update(&identifier, 1024).unwrap(),
            b"updated mixer"
        );
        assert!(matches!(
            writer.external_body_for_update(&identifier, 2),
            Err(Error::LimitExceeded { .. })
        ));
        assert_eq!(
            writer
                .replace_or_update_external_body(&identifier, previous)
                .unwrap()
                .unwrap(),
            b"updated mixer"
        );
        assert_eq!(
            writer.unstage_new_external_body(&identifier).unwrap(),
            b"animation mixer"
        );
        assert_eq!(writer.addition_count(), 0);
    }

    #[test]
    fn rejects_duplicate_or_limited_external_additions_without_losing_pending_state() {
        let source = sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        assert!(matches!(
            writer.add_external_body(EXTERNAL_ID, b"duplicate".to_vec()),
            Err(Error::InvalidWrite { .. })
        ));
        writer
            .add_external_body(NEW_EXTERNAL_ID, b"first".to_vec())
            .unwrap();
        assert!(matches!(
            writer.add_external_body(NEW_EXTERNAL_ID, b"second".to_vec()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.replace_external_body(NEW_EXTERNAL_ID, b"replacement".to_vec()),
            Err(Error::InvalidWrite { .. })
        ));
        writer
            .database()
            .connection()
            .execute_batch("BEGIN")
            .unwrap();
        assert!(matches!(
            writer.write_to(&mut Vec::new()),
            Err(Error::InvalidWrite { .. })
        ));
        writer
            .database()
            .connection()
            .execute_batch("ROLLBACK")
            .unwrap();
        assert_eq!(writer.addition_count(), 1);
        assert_eq!(
            writer.remove_external_addition(NEW_EXTERNAL_ID).unwrap(),
            b"first"
        );

        let source = sample();
        let mut clip = ClipFile::open_with_limits(
            Cursor::new(source),
            crate::Limits::default()
                .with_max_top_level_chunks(4)
                .with_max_write_external_body_size(2),
        )
        .unwrap();
        let mut writer = clip.writer().unwrap();
        assert!(matches!(
            writer.add_external_body(NEW_EXTERNAL_ID, b"abc".to_vec()),
            Err(Error::LimitExceeded { .. })
        ));
        assert!(matches!(
            writer.add_external_body(NEW_EXTERNAL_ID, b"ok".to_vec()),
            Err(Error::LimitExceeded {
                resource: "top-level chunks after external addition",
                ..
            })
        ));
        assert_eq!(writer.addition_count(), 0);
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
    fn reencodes_one_block_and_builds_on_a_pending_replacement() {
        let source = sample_with_external_body(&test_block_body(Some(&[1, 2, 3, 4]), 0x1234));
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        let first = writer
            .replace_block_bytes(EXTERNAL_ID, 0, [4, 3, 2, 1], BlockChecksumMode::Zero)
            .unwrap();
        assert_eq!(first.block_index(), 0);
        assert_eq!(first.decoded_size(), 4);
        assert_eq!(first.original_checksum(), 0x1234);
        assert!(first.original_compressed_size().is_some());
        assert!(matches!(
            writer.replace_block_bytes(EXTERNAL_ID, 0, [1, 2, 3], BlockChecksumMode::Zero),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 1);
        let second = writer
            .replace_block_bytes(EXTERNAL_ID, 0, [9, 8, 7, 6], BlockChecksumMode::Zero)
            .unwrap();
        assert_eq!(second.original_checksum(), 0);
        assert_eq!(writer.replacement_count(), 1);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        rewritten.validate().unwrap();
        let external = rewritten
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::External).then_some(chunk)
            })
            .unwrap();
        let object = rewritten.inspect_external_chunk(&external).unwrap();
        let block_data = rewritten.read_block_data(&object).unwrap();
        let block = &block_data.blocks()[0];
        assert_eq!(block.status(), 1);
        assert_eq!(block.checksum(), 0);
        let compressed = rewritten
            .read_block_payload(block.payload().unwrap(), 1024)
            .unwrap();
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, [9, 8, 7, 6]);
    }

    #[test]
    fn reencodes_a_block_with_the_csp_compatible_checksum() {
        let source = sample_with_external_body(&test_block_body(Some(&[1, 2, 3, 4]), 0x1234));
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        writer
            .replace_block_bytes(
                EXTERNAL_ID,
                0,
                [4, 3, 2, 1],
                BlockChecksumMode::CspCompatible,
            )
            .unwrap();

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let external = rewritten
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::External).then_some(chunk)
            })
            .unwrap();
        let object = rewritten.inspect_external_chunk(&external).unwrap();
        let block_data = rewritten.read_block_data(&object).unwrap();
        let block = &block_data.blocks()[0];
        let compressed = rewritten
            .read_block_payload(block.payload().unwrap(), 1024)
            .unwrap();

        let mut a = 1_u32;
        let mut b = 0_u32;
        for byte in (compressed.len() as u32)
            .to_le_bytes()
            .into_iter()
            .chain(compressed.iter().copied())
        {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        assert_eq!(block.checksum(), (b << 16) | a);
        assert_ne!(block.checksum(), 0);
    }

    #[test]
    fn updates_every_offscreen_block_size_alias_when_reencoding() {
        let body = test_block_body(Some(&[1, 2, 3, 4]), 0x1234);
        let original_size = u32::from_be_bytes(body[..4].try_into().unwrap());
        let attribute = test_offscreen_attribute(original_size);
        let source = sample_with_external_body_and_offscreen(
            &body,
            &[(10, attribute.clone()), (11, attribute)],
        );
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        let summary = writer
            .replace_block_bytes(
                EXTERNAL_ID,
                0,
                [4, 3, 2, 1],
                BlockChecksumMode::CspCompatible,
            )
            .unwrap();
        let mut statement = writer
            .database()
            .connection()
            .prepare("SELECT Attribute FROM Offscreen ORDER BY MainId")
            .unwrap();
        let attributes = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(attributes.len(), 2);
        for attribute in attributes {
            assert_eq!(
                OffscreenAttributes::parse(&attribute)
                    .unwrap()
                    .block_sizes(),
                &[summary.block_record_size()]
            );
        }
    }

    #[test]
    fn updates_block_size_for_a_text_storage_class_offscreen_identifier() {
        let body = test_block_body(Some(&[1, 2, 3, 4]), 0x1234);
        let attribute = test_offscreen_attribute(999);
        let source = sample_with_external_body_and_offscreen(&body, &[(10, attribute.clone())]);
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "UPDATE Offscreen SET BlockData = CAST(BlockData AS TEXT) WHERE MainId = 10",
                [],
            )
            .unwrap();
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT typeof(BlockData) FROM Offscreen WHERE MainId = 10",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "text"
        );

        let summary = writer
            .replace_block_bytes(
                EXTERNAL_ID,
                0,
                [4, 3, 2, 1],
                BlockChecksumMode::CspCompatible,
            )
            .unwrap();
        let updated: Vec<u8> = writer
            .database()
            .connection()
            .query_row(
                "SELECT Attribute FROM Offscreen WHERE MainId = 10",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            OffscreenAttributes::parse(&updated).unwrap().block_sizes(),
            &[summary.block_record_size()]
        );
        assert_ne!(summary.block_record_size(), 999);
    }

    #[cfg(feature = "raster")]
    #[test]
    fn rejects_a_layer_raster_shared_with_a_different_semantic_layout() {
        let body = test_block_body(Some(&[1, 2, 3, 4]), 0x1234);
        let original_size = u32::from_be_bytes(body[..4].try_into().unwrap());
        let selected = test_gray_offscreen_attribute(original_size, 2, 2);
        let alias = test_gray_offscreen_attribute(original_size, 1, 2);
        let source = sample_with_external_body_and_offscreen(
            &body,
            &[(10, selected.clone()), (11, alias.clone())],
        );
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.replace_layer_raster_pixels(
                20,
                PixelFormat::Gray8,
                [4, 3, 2, 1],
                BlockChecksumMode::CspCompatible,
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
        let stored = writer
            .database()
            .connection()
            .prepare("SELECT Attribute FROM Offscreen ORDER BY MainId")
            .unwrap()
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(stored, [selected, alias]);
    }

    #[test]
    fn rolls_back_offscreen_updates_when_main_ids_are_not_unique() {
        let body = test_block_body(Some(&[1, 2, 3, 4]), 0x1234);
        let original_size = u32::from_be_bytes(body[..4].try_into().unwrap());
        let attribute = test_offscreen_attribute(original_size);
        let source = sample_with_external_body_and_offscreen(
            &body,
            &[(10, attribute.clone()), (10, attribute.clone())],
        );
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.replace_block_bytes(
                EXTERNAL_ID,
                0,
                [4, 3, 2, 1],
                BlockChecksumMode::CspCompatible,
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
        let unchanged: i64 = writer
            .database()
            .connection()
            .query_row(
                "SELECT count(*) FROM Offscreen WHERE Attribute = ?1",
                params![attribute],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unchanged, 2);
    }

    #[test]
    fn populates_an_empty_block() {
        let source = sample_with_external_body(&test_block_body(None, 0x1234));
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        let summary = writer
            .replace_block_bytes(EXTERNAL_ID, 0, [1, 2, 3, 4], BlockChecksumMode::Zero)
            .unwrap();
        assert_eq!(summary.original_compressed_size(), None);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let external = rewritten
            .chunks()
            .find_map(|chunk| {
                let chunk = chunk.unwrap();
                (chunk.kind() == ChunkKind::External).then_some(chunk)
            })
            .unwrap();
        let object = rewritten.inspect_external_chunk(&external).unwrap();
        let block_data = rewritten.read_block_data(&object).unwrap();
        let block = &block_data.blocks()[0];
        let compressed = rewritten
            .read_block_payload(block.payload().unwrap(), 1024)
            .unwrap();
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, [1, 2, 3, 4]);
        assert_eq!(block.checksum(), 0);
    }

    #[test]
    fn rejects_a_block_replacement_with_the_wrong_decoded_size() {
        let source = sample_with_external_body(&test_block_body(Some(&[1, 2, 3, 4]), 0x1234));
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.replace_block_bytes(EXTERNAL_ID, 0, [1, 2, 3], BlockChecksumMode::Zero),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
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
