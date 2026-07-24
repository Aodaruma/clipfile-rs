use std::io::{Read, Seek, SeekFrom};

use flate2::read::ZlibDecoder;
use rusqlite::{OptionalExtension, params, types::ValueRef};

use crate::{Block, BlockParameters, ChunkKind, ClipFile, Database, Error, ExternalBody, Result};

/// The sixteen big-endian values in an `Offscreen.Attribute` packing record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelPacking {
    raw: [u32; 16],
}

impl PixelPacking {
    /// All values, including fields whose semantics are not yet understood.
    #[must_use]
    pub const fn raw(&self) -> [u32; 16] {
        self.raw
    }

    /// Number of leading planar alpha channels.
    #[must_use]
    pub const fn alpha_channels(&self) -> u32 {
        self.raw[1]
    }

    /// Number of interleaved color-buffer channels.
    #[must_use]
    pub const fn buffer_channels(&self) -> u32 {
        self.raw[2]
    }

    /// Total channel count recorded by the format.
    #[must_use]
    pub const fn total_channels(&self) -> u32 {
        self.raw[3]
    }

    /// Total interleaved color-buffer depth in bits per pixel.
    #[must_use]
    pub const fn buffer_bit_depth(&self) -> u32 {
        self.raw[6] >> 5
    }

    /// Color-buffer depth per channel when evenly divisible.
    #[must_use]
    pub const fn buffer_bits_per_channel(&self) -> Option<u32> {
        if self.raw[2] == 0 || self.buffer_bit_depth() % self.raw[2] != 0 {
            None
        } else {
            Some(self.buffer_bit_depth() / self.raw[2])
        }
    }

    /// Alpha depth in bits after decoding the stored fixed-point value.
    #[must_use]
    pub const fn alpha_bit_depth(&self) -> u32 {
        self.raw[8] >> 5
    }

    /// Width of one tile in pixels.
    #[must_use]
    pub const fn block_width(&self) -> u32 {
        self.raw[10]
    }

    /// Height of one tile in pixels.
    #[must_use]
    pub const fn block_height(&self) -> u32 {
        self.raw[11]
    }

    /// Whether the format marks the buffer as monochrome/bit-packed.
    #[must_use]
    pub const fn is_monochrome(&self) -> bool {
        self.raw[14] != 0
    }
}

/// Parsed metadata from an SQLite `Offscreen.Attribute` BLOB.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OffscreenAttributes {
    bitmap_width: u32,
    bitmap_height: u32,
    block_grid_width: u32,
    block_grid_height: u32,
    packing: PixelPacking,
    default_fill: u32,
    initial_colors: Vec<u32>,
    block_sizes: Vec<u32>,
}

impl OffscreenAttributes {
    /// Parses the complete `Parameter`, `InitColor`, and `BlockSize` sections.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        parse_attributes(bytes)
    }

    /// Bitmap width in pixels.
    #[must_use]
    pub const fn bitmap_width(&self) -> u32 {
        self.bitmap_width
    }

    /// Bitmap height in pixels.
    #[must_use]
    pub const fn bitmap_height(&self) -> u32 {
        self.bitmap_height
    }

    /// Number of tile columns.
    #[must_use]
    pub const fn block_grid_width(&self) -> u32 {
        self.block_grid_width
    }

    /// Number of tile rows.
    #[must_use]
    pub const fn block_grid_height(&self) -> u32 {
        self.block_grid_height
    }

    /// Pixel packing metadata.
    #[must_use]
    pub const fn packing(&self) -> PixelPacking {
        self.packing
    }

    /// Opaque default-fill value; observed values are zero and one.
    #[must_use]
    pub const fn default_fill(&self) -> u32 {
        self.default_fill
    }

    /// Additional initialization color values.
    #[must_use]
    pub fn initial_colors(&self) -> &[u32] {
        &self.initial_colors
    }

    /// Per-block size metadata from the attribute record.
    #[must_use]
    pub fn block_sizes(&self) -> &[u32] {
        &self.block_sizes
    }
}

/// SQLite references needed to read one base mipmap raster.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterSource {
    mipmap_id: i64,
    offscreen_id: i64,
    layer_id: Option<i64>,
    external_identifier: Option<Box<[u8]>>,
    attributes: OffscreenAttributes,
}

impl RasterSource {
    /// `Mipmap.MainId` used to resolve this source.
    #[must_use]
    pub const fn mipmap_id(&self) -> i64 {
        self.mipmap_id
    }

    /// Resolved `Offscreen.MainId`.
    #[must_use]
    pub const fn offscreen_id(&self) -> i64 {
        self.offscreen_id
    }

    /// Owning layer ID, if the row contains one.
    #[must_use]
    pub const fn layer_id(&self) -> Option<i64> {
        self.layer_id
    }

    /// External block-data identifier, if one is recorded.
    #[must_use]
    pub fn external_identifier(&self) -> Option<&[u8]> {
        self.external_identifier.as_deref()
    }

    /// Parsed offscreen attributes.
    #[must_use]
    pub const fn attributes(&self) -> &OffscreenAttributes {
        &self.attributes
    }
}

/// Pixel layout of decoded raster bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Interleaved red, green, blue, alpha bytes.
    Rgba8,
    /// One eight-bit grayscale channel.
    Gray8,
}

impl PixelFormat {
    const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8 => 4,
            Self::Gray8 => 1,
        }
    }
}

/// Whether decoded pixels had a corresponding external object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RasterDataState {
    /// The offscreen row does not record a block-data identifier.
    MissingReference,
    /// An identifier is recorded but absent from `ExternalChunk`.
    MissingExternalChunk,
    /// The external block-data object was found and decoded.
    Present,
}

impl RasterDataState {
    /// Whether the raster consists only of the `Offscreen.Attribute`
    /// default fill because no external block-data object was resolved.
    #[must_use]
    pub const fn is_default_filled(self) -> bool {
        matches!(self, Self::MissingReference | Self::MissingExternalChunk)
    }

    /// Whether an external block-data object was found and decoded.
    #[must_use]
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

/// One zlib-decoded tile in the format's native channel arrangement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedTile {
    index: u32,
    parameters: BlockParameters,
    bytes: Vec<u8>,
}

impl DecodedTile {
    /// Tile-grid index.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Channel count and tile dimensions recorded by the block.
    #[must_use]
    pub const fn parameters(&self) -> BlockParameters {
        self.parameters
    }

    /// Native decoded bytes: planar alpha followed by the color buffer.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Takes ownership of the native decoded bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A fully assembled raster bitmap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterImage {
    width: u32,
    height: u32,
    format: PixelFormat,
    state: RasterDataState,
    pixels: Vec<u8>,
}

impl RasterImage {
    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format of [`Self::pixels`].
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// State of the external-data resolution.
    #[must_use]
    pub const fn data_state(&self) -> RasterDataState {
        self.state
    }

    /// Contiguous row-major pixels.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Takes ownership of the row-major pixels.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

impl Database {
    /// Resolves a mipmap through `MipmapInfo` to its base `Offscreen` row.
    pub fn raster_source(&self, mipmap_id: i64) -> Result<Option<RasterSource>> {
        for (table, columns) in [
            ("Mipmap", &["MainId", "BaseMipmapInfo"][..]),
            ("MipmapInfo", &["MainId", "Offscreen"][..]),
            (
                "Offscreen",
                &["MainId", "LayerId", "Attribute", "BlockData"][..],
            ),
        ] {
            for column in columns {
                self.require_column(table, column)?;
            }
        }
        let raw = self
            .connection()
            .query_row(
                "SELECT o.MainId, o.LayerId, o.Attribute, o.BlockData \
                 FROM Mipmap AS m \
                 JOIN MipmapInfo AS mi ON mi.MainId = m.BaseMipmapInfo \
                 JOIN Offscreen AS o ON o.MainId = mi.Offscreen \
                 WHERE m.MainId = ?1 LIMIT 1",
                params![mipmap_id],
                |row| {
                    let offscreen_id = row.get(0)?;
                    let layer_id = row.get(1)?;
                    let attributes = value_bytes(row.get_ref(2)?, 2, "Attribute")?;
                    let external_identifier = match row.get_ref(3)? {
                        ValueRef::Null => None,
                        value => Some(value_bytes(value, 3, "BlockData")?),
                    };
                    Ok((offscreen_id, layer_id, attributes, external_identifier))
                },
            )
            .optional()?;
        let Some((offscreen_id, layer_id, attributes, external_identifier)) = raw else {
            return Ok(None);
        };
        Ok(Some(RasterSource {
            mipmap_id,
            offscreen_id,
            layer_id,
            external_identifier,
            attributes: OffscreenAttributes::parse(&attributes)?,
        }))
    }

    /// Resolves the render mipmap for one layer.
    pub fn layer_raster_source(&self, layer_id: i64) -> Result<Option<RasterSource>> {
        self.require_column("Layer", "MainId")?;
        self.require_column("Layer", "LayerRenderMipmap")?;
        let mipmap_id = self
            .connection()
            .query_row(
                "SELECT LayerRenderMipmap FROM Layer WHERE MainId = ?1 LIMIT 1",
                params![layer_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        match mipmap_id {
            Some(id) if id != 0 => self.raster_source(id),
            _ => Ok(None),
        }
    }

    /// Resolves the layer-mask mipmap for one layer.
    pub fn layer_mask_raster_source(&self, layer_id: i64) -> Result<Option<RasterSource>> {
        self.require_column("Layer", "MainId")?;
        self.require_column("Layer", "LayerLayerMaskMipmap")?;
        let mipmap_id = self
            .connection()
            .query_row(
                "SELECT LayerLayerMaskMipmap FROM Layer WHERE MainId = ?1 LIMIT 1",
                params![layer_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        match mipmap_id {
            Some(id) if id != 0 => self.raster_source(id),
            _ => Ok(None),
        }
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Decompresses one present block under the configured size limit.
    pub fn decode_tile(&mut self, block: &Block) -> Result<Option<DecodedTile>> {
        let Some(payload) = block.payload() else {
            return Ok(None);
        };
        let parameters = block.parameters();
        let expected = u64::from(parameters.channel_count())
            .checked_mul(u64::from(parameters.width()))
            .and_then(|value| value.checked_mul(u64::from(parameters.height())))
            .ok_or(Error::OffsetOverflow)?;
        let limit = self.limits.max_decompressed_block_size();
        if expected > limit {
            return Err(Error::LimitExceeded {
                resource: "decompressed block size",
                value: expected,
                limit,
            });
        }
        let bytes = decode_zlib_range(
            &mut self.reader,
            self.file_size,
            payload.offset(),
            payload.compressed_size(),
            expected,
            limit,
        )?;
        Ok(Some(DecodedTile {
            index: block.index(),
            parameters,
            bytes,
        }))
    }

    /// Decodes and assembles a raster source into row-major pixels.
    pub fn decode_raster(
        &mut self,
        database: &Database,
        source: &RasterSource,
    ) -> Result<RasterImage> {
        let attributes = source.attributes();
        validate_dimensions(attributes, self.limits.max_canvas_dimension())?;
        let format = pixel_format(attributes.packing())?;
        let allocation = u64::from(attributes.bitmap_width())
            .checked_mul(u64::from(attributes.bitmap_height()))
            .and_then(|pixels| pixels.checked_mul(format.bytes_per_pixel()))
            .ok_or(Error::OffsetOverflow)?;
        let allocation_limit = self.limits.max_raster_bytes();
        if allocation > allocation_limit {
            return Err(Error::LimitExceeded {
                resource: "decoded raster bytes",
                value: allocation,
                limit: allocation_limit,
            });
        }
        let fill = if attributes.default_fill() == 0 {
            0
        } else {
            u8::MAX
        };
        let mut image = RasterImage {
            width: attributes.bitmap_width(),
            height: attributes.bitmap_height(),
            format,
            state: RasterDataState::MissingReference,
            pixels: vec![fill; usize::try_from(allocation).map_err(|_| Error::OffsetOverflow)?],
        };
        let Some(identifier) = source.external_identifier() else {
            return Ok(image);
        };
        let Some(record) = database.external_chunk(identifier)? else {
            image.state = RasterDataState::MissingExternalChunk;
            return Ok(image);
        };
        let chunk = self.chunk_at_offset(record.offset())?;
        if chunk.kind() != ChunkKind::External {
            return Err(Error::InvalidRaster {
                reason: format!("external index offset {} is not CHNKExta", record.offset()),
            });
        }
        let object = self.inspect_external_chunk(&chunk)?;
        if object.header().identifier() != identifier {
            return Err(Error::InvalidRaster {
                reason: "resolved external identifier does not match CHNKExta".to_owned(),
            });
        }
        if object.body() != ExternalBody::BlockData {
            return Err(Error::UnsupportedRaster {
                reason: "Offscreen.BlockData does not refer to block data".to_owned(),
            });
        }
        let blocks = self.read_block_data(&object)?;
        let expected_blocks = u64::from(attributes.block_grid_width())
            .checked_mul(u64::from(attributes.block_grid_height()))
            .ok_or(Error::OffsetOverflow)?;
        if blocks.blocks().len() as u64 != expected_blocks {
            return Err(Error::InvalidRaster {
                reason: format!(
                    "attribute grid requires {expected_blocks} blocks, external object contains {}",
                    blocks.blocks().len()
                ),
            });
        }
        let mut seen = vec![false; blocks.blocks().len()];
        for block in blocks.blocks() {
            let index = usize::try_from(block.index()).map_err(|_| Error::OffsetOverflow)?;
            let Some(slot) = seen.get_mut(index) else {
                return Err(Error::InvalidRaster {
                    reason: format!("tile index {} is outside the attribute grid", block.index()),
                });
            };
            if *slot {
                return Err(Error::InvalidRaster {
                    reason: format!("duplicate tile index {}", block.index()),
                });
            }
            *slot = true;
            if let Some(tile) = self.decode_tile(block)? {
                copy_tile(&mut image, attributes, &tile)?;
            }
        }
        image.state = RasterDataState::Present;
        Ok(image)
    }
}

fn value_bytes(value: ValueRef<'_>, index: usize, name: &str) -> rusqlite::Result<Box<[u8]>> {
    match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(Box::from(bytes)),
        _ => Err(rusqlite::Error::InvalidColumnType(
            index,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn parse_attributes(bytes: &[u8]) -> Result<OffscreenAttributes> {
    let mut reader = AttributeReader::new(bytes);
    let header_size = reader.u32()?;
    let parameter_size = reader.u32()?;
    let init_color_size = reader.u32()?;
    let block_size = reader.u32()?;
    if header_size != 16 {
        return invalid_raster(format!(
            "attribute header size is {header_size}, expected 16"
        ));
    }
    let total = u64::from(header_size)
        .checked_add(u64::from(parameter_size))
        .and_then(|value| value.checked_add(u64::from(init_color_size)))
        .and_then(|value| value.checked_add(u64::from(block_size)))
        .ok_or(Error::OffsetOverflow)?;
    if total != bytes.len() as u64 {
        return invalid_raster(format!(
            "attribute sections total {total} bytes, BLOB contains {}",
            bytes.len()
        ));
    }

    let parameter_end = section_end(reader.position(), parameter_size, bytes.len())?;
    reader.label("Parameter")?;
    let bitmap_width = reader.u32()?;
    let bitmap_height = reader.u32()?;
    let block_grid_width = reader.u32()?;
    let block_grid_height = reader.u32()?;
    let mut packing = [0_u32; 16];
    for value in &mut packing {
        *value = reader.u32()?;
    }
    reader.require_position(parameter_end, "Parameter")?;

    let init_end = section_end(reader.position(), init_color_size, bytes.len())?;
    reader.label("InitColor")?;
    let init_record_size = reader.u32()?;
    if init_record_size != 20 {
        return invalid_raster(format!(
            "InitColor record size is {init_record_size}, expected 20"
        ));
    }
    let default_fill = reader.u32()?;
    let _unknown = reader.u32()?;
    let initial_color_count = reader.u32()?;
    let initial_color_width = reader.u32()?;
    if initial_color_width != 4 {
        return invalid_raster(format!(
            "InitColor element size is {initial_color_width}, expected 4"
        ));
    }
    let mut initial_colors = Vec::with_capacity(bounded_count(
        initial_color_count,
        reader.remaining_to(init_end)?,
    )?);
    for _ in 0..initial_color_count {
        initial_colors.push(reader.u32()?);
    }
    reader.require_position(init_end, "InitColor")?;

    let block_end = section_end(reader.position(), block_size, bytes.len())?;
    reader.label("BlockSize")?;
    let block_record_size = reader.u32()?;
    if block_record_size != 12 {
        return invalid_raster(format!(
            "BlockSize record size is {block_record_size}, expected 12"
        ));
    }
    let block_count = reader.u32()?;
    let block_element_width = reader.u32()?;
    if block_element_width != 4 {
        return invalid_raster(format!(
            "BlockSize element size is {block_element_width}, expected 4"
        ));
    }
    let mut block_sizes =
        Vec::with_capacity(bounded_count(block_count, reader.remaining_to(block_end)?)?);
    for _ in 0..block_count {
        block_sizes.push(reader.u32()?);
    }
    reader.require_position(block_end, "BlockSize")?;
    let grid_blocks = u64::from(block_grid_width)
        .checked_mul(u64::from(block_grid_height))
        .ok_or(Error::OffsetOverflow)?;
    if u64::from(block_count) != grid_blocks {
        return invalid_raster(format!(
            "attribute grid requires {grid_blocks} BlockSize entries, found {block_count}"
        ));
    }
    Ok(OffscreenAttributes {
        bitmap_width,
        bitmap_height,
        block_grid_width,
        block_grid_height,
        packing: PixelPacking { raw: packing },
        default_fill,
        initial_colors,
        block_sizes,
    })
}

fn pixel_format(packing: PixelPacking) -> Result<PixelFormat> {
    let alpha = packing.alpha_channels();
    let buffer = packing.buffer_channels();
    if alpha.checked_add(buffer) != Some(packing.total_channels()) {
        return Err(Error::InvalidRaster {
            reason: format!(
                "packing channel sum ({alpha} + {buffer}) does not match total {}",
                packing.total_channels()
            ),
        });
    }
    let depth_is_eight = (alpha == 0 || packing.alpha_bit_depth() == 8)
        && (buffer == 0 || packing.buffer_bits_per_channel() == Some(8));
    if !depth_is_eight || packing.is_monochrome() {
        return Err(Error::UnsupportedRaster {
            reason: format!(
                "only non-bit-packed 8-bit channels are supported (alpha={alpha} at {}-bit, \
                 buffer={buffer} at {:?}-bit/channel, monochrome={})",
                packing.alpha_bit_depth(),
                packing.buffer_bits_per_channel(),
                packing.is_monochrome()
            ),
        });
    }
    match (alpha, buffer) {
        (1, 4) => Ok(PixelFormat::Rgba8),
        (1, 0) | (0, 1) => Ok(PixelFormat::Gray8),
        _ => Err(Error::UnsupportedRaster {
            reason: format!("unsupported channel packing ({alpha}, {buffer})"),
        }),
    }
}

fn validate_dimensions(attributes: &OffscreenAttributes, limit: u32) -> Result<()> {
    for (resource, value) in [
        ("raster width", attributes.bitmap_width()),
        ("raster height", attributes.bitmap_height()),
        ("tile width", attributes.packing().block_width()),
        ("tile height", attributes.packing().block_height()),
    ] {
        if value == 0 {
            return invalid_raster(format!("{resource} is zero"));
        }
        if value > limit {
            return Err(Error::LimitExceeded {
                resource,
                value: u64::from(value),
                limit: u64::from(limit),
            });
        }
    }
    Ok(())
}

fn copy_tile(
    image: &mut RasterImage,
    attributes: &OffscreenAttributes,
    tile: &DecodedTile,
) -> Result<()> {
    let packing = attributes.packing();
    let parameters = tile.parameters();
    let channels = packing
        .alpha_channels()
        .checked_add(packing.buffer_channels())
        .ok_or(Error::OffsetOverflow)?;
    if u32::from(parameters.channel_count()) != channels
        || parameters.width() != packing.block_width()
        || parameters.height() != packing.block_height()
    {
        return invalid_raster(format!(
            "tile {} parameters do not match Offscreen.Attribute",
            tile.index()
        ));
    }
    let grid_width = attributes.block_grid_width();
    let tile_x = tile.index() % grid_width;
    let tile_y = tile.index() / grid_width;
    let origin_x = tile_x
        .checked_mul(parameters.width())
        .ok_or(Error::OffsetOverflow)?;
    let origin_y = tile_y
        .checked_mul(parameters.height())
        .ok_or(Error::OffsetOverflow)?;
    let copy_width = parameters.width().min(image.width.saturating_sub(origin_x));
    let copy_height = parameters
        .height()
        .min(image.height.saturating_sub(origin_y));
    let tile_area = u64::from(parameters.width())
        .checked_mul(u64::from(parameters.height()))
        .ok_or(Error::OffsetOverflow)?;
    for y in 0..copy_height {
        for x in 0..copy_width {
            let source_pixel = u64::from(y)
                .checked_mul(u64::from(parameters.width()))
                .and_then(|value| value.checked_add(u64::from(x)))
                .ok_or(Error::OffsetOverflow)?;
            let target_pixel = u64::from(origin_y + y)
                .checked_mul(u64::from(image.width))
                .and_then(|value| value.checked_add(u64::from(origin_x + x)))
                .ok_or(Error::OffsetOverflow)?;
            match image.format {
                PixelFormat::Rgba8 => {
                    let alpha = byte_at(&tile.bytes, source_pixel)?;
                    let buffer = tile_area
                        .checked_add(source_pixel.checked_mul(4).ok_or(Error::OffsetOverflow)?)
                        .ok_or(Error::OffsetOverflow)?;
                    let target = target_pixel.checked_mul(4).ok_or(Error::OffsetOverflow)?;
                    let target = usize::try_from(target).map_err(|_| Error::OffsetOverflow)?;
                    image.pixels[target] = byte_at(&tile.bytes, buffer + 2)?;
                    image.pixels[target + 1] = byte_at(&tile.bytes, buffer + 1)?;
                    image.pixels[target + 2] = byte_at(&tile.bytes, buffer)?;
                    image.pixels[target + 3] = alpha;
                }
                PixelFormat::Gray8 => {
                    let target =
                        usize::try_from(target_pixel).map_err(|_| Error::OffsetOverflow)?;
                    image.pixels[target] = byte_at(&tile.bytes, source_pixel)?;
                }
            }
        }
    }
    Ok(())
}

fn byte_at(bytes: &[u8], offset: u64) -> Result<u8> {
    bytes
        .get(usize::try_from(offset).map_err(|_| Error::OffsetOverflow)?)
        .copied()
        .ok_or_else(|| Error::InvalidRaster {
            reason: "decoded tile is shorter than its channel layout".to_owned(),
        })
}

fn decode_zlib_range<R: Read + Seek>(
    reader: &mut R,
    file_size: u64,
    offset: u64,
    compressed_size: u64,
    expected_size: u64,
    limit: u64,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(compressed_size)
        .ok_or(Error::OffsetOverflow)?;
    if end > file_size {
        return invalid_raster("compressed tile extends beyond the file".to_owned());
    }
    reader.seek(SeekFrom::Start(offset))?;
    let source = reader.by_ref().take(compressed_size);
    let decoder = ZlibDecoder::new(source);
    let mut bounded = decoder.take(limit.saturating_add(1));
    let capacity = usize::try_from(expected_size).map_err(|_| Error::OffsetOverflow)?;
    let mut bytes = Vec::with_capacity(capacity);
    bounded.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::LimitExceeded {
            resource: "decompressed block size",
            value: bytes.len() as u64,
            limit,
        });
    }
    if bytes.len() as u64 != expected_size {
        return invalid_raster(format!(
            "tile expands to {} bytes, expected {expected_size}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

fn section_end(start: usize, size: u32, total: usize) -> Result<usize> {
    let end = start
        .checked_add(usize::try_from(size).map_err(|_| Error::OffsetOverflow)?)
        .ok_or(Error::OffsetOverflow)?;
    if end > total {
        return invalid_raster("attribute section extends beyond the BLOB".to_owned());
    }
    Ok(end)
}

fn bounded_count(count: u32, remaining: usize) -> Result<usize> {
    let count = usize::try_from(count).map_err(|_| Error::OffsetOverflow)?;
    if count > remaining / 4 {
        return invalid_raster("attribute array exceeds its section".to_owned());
    }
    Ok(count)
}

fn invalid_raster<T>(reason: String) -> Result<T> {
    Err(Error::InvalidRaster { reason })
}

struct AttributeReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> AttributeReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    fn u32(&mut self) -> Result<u32> {
        let end = self.position.checked_add(4).ok_or(Error::OffsetOverflow)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| Error::InvalidRaster {
                reason: "unexpected end of Offscreen.Attribute".to_owned(),
            })?;
        self.position = end;
        Ok(u32::from_be_bytes(
            bytes.try_into().expect("four-byte slice"),
        ))
    }

    fn label(&mut self, expected: &str) -> Result<()> {
        let characters = usize::try_from(self.u32()?).map_err(|_| Error::OffsetOverflow)?;
        let byte_count = characters.checked_mul(2).ok_or(Error::OffsetOverflow)?;
        let end = self
            .position
            .checked_add(byte_count)
            .ok_or(Error::OffsetOverflow)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| Error::InvalidRaster {
                reason: "attribute label extends beyond the BLOB".to_owned(),
            })?;
        let decoded = String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| Error::InvalidRaster {
            reason: "attribute label is not valid UTF-16BE".to_owned(),
        })?;
        self.position = end;
        if decoded != expected {
            return invalid_raster(format!(
                "attribute label is {decoded:?}, expected {expected:?}"
            ));
        }
        Ok(())
    }

    fn remaining_to(&self, end: usize) -> Result<usize> {
        end.checked_sub(self.position)
            .ok_or_else(|| Error::InvalidRaster {
                reason: "attribute parser crossed a section boundary".to_owned(),
            })
    }

    fn require_position(&self, expected: usize, section: &str) -> Result<()> {
        if self.position == expected {
            Ok(())
        } else {
            invalid_raster(format!(
                "{section} section ended at {}, expected {expected}",
                self.position
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::ZlibEncoder};
    use rusqlite::{Connection, params};

    use super::*;

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_label(bytes: &mut Vec<u8>, value: &str) {
        push_u32(bytes, value.encode_utf16().count() as u32);
        for character in value.encode_utf16() {
            bytes.extend_from_slice(&character.to_be_bytes());
        }
    }

    fn attributes() -> Vec<u8> {
        let mut parameter = Vec::new();
        push_label(&mut parameter, "Parameter");
        for value in [300, 200, 2, 1] {
            push_u32(&mut parameter, value);
        }
        let mut packing = [0_u32; 16];
        packing[1] = 1;
        packing[2] = 4;
        packing[3] = 5;
        packing[6] = 32 << 5;
        packing[8] = 8 << 5;
        packing[10] = 256;
        packing[11] = 256;
        for value in packing {
            push_u32(&mut parameter, value);
        }

        let mut init = Vec::new();
        push_label(&mut init, "InitColor");
        for value in [20, 0, 0, 0, 4] {
            push_u32(&mut init, value);
        }

        let mut blocks = Vec::new();
        push_label(&mut blocks, "BlockSize");
        for value in [12, 2, 4, 104, 104] {
            push_u32(&mut blocks, value);
        }

        let mut bytes = Vec::new();
        for value in [
            16,
            parameter.len() as u32,
            init.len() as u32,
            blocks.len() as u32,
        ] {
            push_u32(&mut bytes, value);
        }
        bytes.extend(parameter);
        bytes.extend(init);
        bytes.extend(blocks);
        bytes
    }

    fn raster_database() -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER,
                    LayerRenderMipmap INTEGER,
                    LayerLayerMaskMipmap INTEGER
                 );
                 INSERT INTO Layer VALUES (1, 10, 20);
                 INSERT INTO Layer VALUES (2, 0, 0);
                 CREATE TABLE Mipmap (MainId INTEGER, BaseMipmapInfo INTEGER);
                 INSERT INTO Mipmap VALUES (10, 100);
                 INSERT INTO Mipmap VALUES (20, 200);
                 CREATE TABLE MipmapInfo (MainId INTEGER, Offscreen INTEGER);
                 INSERT INTO MipmapInfo VALUES (100, 1000);
                 INSERT INTO MipmapInfo VALUES (200, 2000);
                 CREATE TABLE Offscreen (
                    MainId INTEGER,
                    LayerId INTEGER,
                    Attribute BLOB,
                    BlockData BLOB
                 );",
            )
            .unwrap();
        let attributes = attributes();
        connection
            .execute(
                "INSERT INTO Offscreen VALUES (?1, ?2, ?3, NULL)",
                params![1000_i64, 1_i64, &attributes],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Offscreen VALUES (?1, ?2, ?3, NULL)",
                params![2000_i64, 1_i64, &attributes],
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn parses_complete_attributes() {
        let attributes = OffscreenAttributes::parse(&attributes()).unwrap();
        assert_eq!(attributes.bitmap_width(), 300);
        assert_eq!(attributes.bitmap_height(), 200);
        assert_eq!(attributes.block_sizes(), &[104, 104]);
        assert_eq!(attributes.packing().alpha_channels(), 1);
        assert_eq!(attributes.packing().buffer_channels(), 4);
        assert_eq!(
            pixel_format(attributes.packing()).unwrap(),
            PixelFormat::Rgba8
        );
    }

    #[test]
    fn rejects_attribute_section_size_mismatch() {
        let mut bytes = attributes();
        bytes[7] = bytes[7].wrapping_add(1);
        assert!(matches!(
            OffscreenAttributes::parse(&bytes),
            Err(Error::InvalidRaster { .. })
        ));
    }

    #[test]
    fn resolves_render_and_mask_sources_for_a_layer() {
        let database = raster_database();
        let render = database.layer_raster_source(1).unwrap().unwrap();
        let mask = database.layer_mask_raster_source(1).unwrap().unwrap();
        assert_eq!(render.mipmap_id(), 10);
        assert_eq!(render.offscreen_id(), 1000);
        assert_eq!(mask.mipmap_id(), 20);
        assert_eq!(mask.offscreen_id(), 2000);
        assert!(database.layer_mask_raster_source(2).unwrap().is_none());
        assert!(database.layer_mask_raster_source(999).unwrap().is_none());
    }

    #[test]
    fn classifies_raster_data_states() {
        assert!(RasterDataState::MissingReference.is_default_filled());
        assert!(RasterDataState::MissingExternalChunk.is_default_filled());
        assert!(!RasterDataState::Present.is_default_filled());
        assert!(!RasterDataState::MissingReference.is_present());
        assert!(!RasterDataState::MissingExternalChunk.is_present());
        assert!(RasterDataState::Present.is_present());
    }

    #[test]
    fn decompresses_only_the_expected_size() {
        let raw = vec![7_u8; 1024];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut source = std::io::Cursor::new(compressed.clone());
        let decoded = decode_zlib_range(
            &mut source,
            compressed.len() as u64,
            0,
            compressed.len() as u64,
            raw.len() as u64,
            raw.len() as u64,
        )
        .unwrap();
        assert_eq!(decoded, raw);
    }
}
