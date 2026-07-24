#[cfg(all(feature = "write", feature = "raster"))]
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};

use flate2::read::ZlibDecoder;
use rusqlite::{OptionalExtension, params, types::ValueRef};
#[cfg(all(feature = "write", feature = "raster"))]
use rusqlite::{params_from_iter, types::Value};

use crate::{Block, BlockParameters, ChunkKind, ClipFile, Database, Error, ExternalBody, Result};
#[cfg(all(feature = "write", feature = "raster"))]
use crate::{
    ClipWriter, Limits,
    external::{BlockChecksumPolicy, rebuild_block_data_body_batch},
};

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
    /// Interleaved eight-bit grayscale value and alpha bytes.
    GrayAlpha8,
}

impl PixelFormat {
    const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8 => 4,
            Self::Gray8 => 1,
            Self::GrayAlpha8 => 2,
        }
    }
}

/// One row-major RGBA8 pixel.
///
/// The public fields make format-aware editing possible without indexing a
/// raw byte chunk. Use [`Self::invert`] to invert color while preserving
/// alpha.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgba8Pixel {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

impl Rgba8Pixel {
    /// Minimum value of each channel.
    pub const CHANNEL_MIN: u8 = u8::MIN;
    /// Maximum value of each channel.
    pub const CHANNEL_MAX: u8 = u8::MAX;
    /// Pixel with every channel at its minimum.
    pub const MIN: Self = Self {
        r: Self::CHANNEL_MIN,
        g: Self::CHANNEL_MIN,
        b: Self::CHANNEL_MIN,
        a: Self::CHANNEL_MIN,
    };
    /// Pixel with every channel at its maximum.
    pub const MAX: Self = Self {
        r: Self::CHANNEL_MAX,
        g: Self::CHANNEL_MAX,
        b: Self::CHANNEL_MAX,
        a: Self::CHANNEL_MAX,
    };

    /// Inverts red, green, and blue while preserving alpha.
    pub const fn invert(&mut self) {
        self.r = Self::CHANNEL_MAX - self.r;
        self.g = Self::CHANNEL_MAX - self.g;
        self.b = Self::CHANNEL_MAX - self.b;
    }

    /// Inverts red, green, and blue while preserving alpha.
    ///
    /// This explicit alias is equivalent to [`Self::invert`].
    pub const fn invert_rgb(&mut self) {
        self.invert();
    }

    /// Adds one value to RGB, returning `None` if any channel would overflow.
    #[must_use]
    pub fn checked_add_rgb(self, value: u8) -> Option<Self> {
        Some(Self {
            r: self.r.checked_add(value)?,
            g: self.g.checked_add(value)?,
            b: self.b.checked_add(value)?,
            a: self.a,
        })
    }

    /// Subtracts one value from RGB, returning `None` if any channel would underflow.
    #[must_use]
    pub fn checked_sub_rgb(self, value: u8) -> Option<Self> {
        Some(Self {
            r: self.r.checked_sub(value)?,
            g: self.g.checked_sub(value)?,
            b: self.b.checked_sub(value)?,
            a: self.a,
        })
    }

    /// Adds one value to RGB and clamps every result to the channel range.
    #[must_use]
    pub const fn saturating_add_rgb(self, value: u8) -> Self {
        Self {
            r: self.r.saturating_add(value),
            g: self.g.saturating_add(value),
            b: self.b.saturating_add(value),
            a: self.a,
        }
    }

    /// Subtracts one value from RGB and clamps every result to the channel range.
    #[must_use]
    pub const fn saturating_sub_rgb(self, value: u8) -> Self {
        Self {
            r: self.r.saturating_sub(value),
            g: self.g.saturating_sub(value),
            b: self.b.saturating_sub(value),
            a: self.a,
        }
    }
}

/// One row-major eight-bit grayscale pixel.
///
/// CLIP raster masks decoded as [`PixelFormat::Gray8`] contain one value
/// channel and no independent alpha channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gray8Pixel {
    /// Grayscale value.
    pub value: u8,
}

impl Gray8Pixel {
    /// Minimum value of the grayscale channel.
    pub const CHANNEL_MIN: u8 = u8::MIN;
    /// Maximum value of the grayscale channel.
    pub const CHANNEL_MAX: u8 = u8::MAX;
    /// Pixel at the minimum value.
    pub const MIN: Self = Self {
        value: Self::CHANNEL_MIN,
    };
    /// Pixel at the maximum value.
    pub const MAX: Self = Self {
        value: Self::CHANNEL_MAX,
    };

    /// Inverts the grayscale value.
    pub const fn invert(&mut self) {
        self.value = Self::CHANNEL_MAX - self.value;
    }

    /// Adds a value, returning `None` if the channel would overflow.
    #[must_use]
    pub const fn checked_add(self, value: u8) -> Option<Self> {
        match self.value.checked_add(value) {
            Some(value) => Some(Self { value }),
            None => None,
        }
    }

    /// Subtracts a value, returning `None` if the channel would underflow.
    #[must_use]
    pub const fn checked_sub(self, value: u8) -> Option<Self> {
        match self.value.checked_sub(value) {
            Some(value) => Some(Self { value }),
            None => None,
        }
    }

    /// Adds a value and clamps the result to the channel range.
    #[must_use]
    pub const fn saturating_add(self, value: u8) -> Self {
        Self {
            value: self.value.saturating_add(value),
        }
    }

    /// Subtracts a value and clamps the result to the channel range.
    #[must_use]
    pub const fn saturating_sub(self, value: u8) -> Self {
        Self {
            value: self.value.saturating_sub(value),
        }
    }
}

/// One row-major eight-bit grayscale pixel with independent alpha.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GrayAlpha8Pixel {
    /// Grayscale value.
    pub value: u8,
    /// Alpha channel.
    pub alpha: u8,
}

impl GrayAlpha8Pixel {
    /// Minimum value of each channel.
    pub const CHANNEL_MIN: u8 = u8::MIN;
    /// Maximum value of each channel.
    pub const CHANNEL_MAX: u8 = u8::MAX;
    /// Pixel with both channels at their minimum.
    pub const MIN: Self = Self {
        value: Self::CHANNEL_MIN,
        alpha: Self::CHANNEL_MIN,
    };
    /// Pixel with both channels at their maximum.
    pub const MAX: Self = Self {
        value: Self::CHANNEL_MAX,
        alpha: Self::CHANNEL_MAX,
    };

    /// Inverts the grayscale value while preserving alpha.
    pub const fn invert(&mut self) {
        self.value = Self::CHANNEL_MAX - self.value;
    }

    /// Inverts the grayscale value while preserving alpha.
    ///
    /// This explicit alias is equivalent to [`Self::invert`].
    pub const fn invert_value(&mut self) {
        self.invert();
    }

    /// Adds to the grayscale value, returning `None` on overflow.
    #[must_use]
    pub const fn checked_add_value(self, value: u8) -> Option<Self> {
        match self.value.checked_add(value) {
            Some(value) => Some(Self {
                value,
                alpha: self.alpha,
            }),
            None => None,
        }
    }

    /// Subtracts from the grayscale value, returning `None` on underflow.
    #[must_use]
    pub const fn checked_sub_value(self, value: u8) -> Option<Self> {
        match self.value.checked_sub(value) {
            Some(value) => Some(Self {
                value,
                alpha: self.alpha,
            }),
            None => None,
        }
    }

    /// Adds to the grayscale value and clamps it to the channel range.
    #[must_use]
    pub const fn saturating_add_value(self, value: u8) -> Self {
        Self {
            value: self.value.saturating_add(value),
            alpha: self.alpha,
        }
    }

    /// Subtracts from the grayscale value and clamps it to the channel range.
    #[must_use]
    pub const fn saturating_sub_value(self, value: u8) -> Self {
        Self {
            value: self.value.saturating_sub(value),
            alpha: self.alpha,
        }
    }
}

/// Immutable iterator over typed RGBA8 pixels.
#[derive(Clone, Debug)]
pub struct Rgba8Pixels<'a> {
    chunks: std::slice::ChunksExact<'a, u8>,
}

impl Iterator for Rgba8Pixels<'_> {
    type Item = Rgba8Pixel;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next()?;
        Some(Rgba8Pixel {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl DoubleEndedIterator for Rgba8Pixels<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next_back()?;
        Some(Rgba8Pixel {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        })
    }
}

impl ExactSizeIterator for Rgba8Pixels<'_> {}
impl std::iter::FusedIterator for Rgba8Pixels<'_> {}

/// Mutable proxy for one RGBA8 pixel.
///
/// Field edits are copied back to the image when this value is dropped. The
/// proxy dereferences to [`Rgba8Pixel`], so fields can be edited directly:
///
/// ```
/// use clipfile::{RasterImage, Rgba8Pixel};
///
/// # fn edit(image: &mut RasterImage) {
/// if let Some(pixels) = image.rgba8_pixels_mut() {
///     for mut pixel in pixels {
///         pixel.r = Rgba8Pixel::CHANNEL_MAX - pixel.r;
///     }
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Rgba8PixelMut<'a> {
    pixel: Rgba8Pixel,
    bytes: &'a mut [u8],
}

impl std::ops::Deref for Rgba8PixelMut<'_> {
    type Target = Rgba8Pixel;

    fn deref(&self) -> &Self::Target {
        &self.pixel
    }
}

impl std::ops::DerefMut for Rgba8PixelMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pixel
    }
}

impl Drop for Rgba8PixelMut<'_> {
    fn drop(&mut self) {
        self.bytes
            .copy_from_slice(&[self.pixel.r, self.pixel.g, self.pixel.b, self.pixel.a]);
    }
}

/// Mutable iterator over typed RGBA8 pixels.
#[derive(Debug)]
pub struct Rgba8PixelsMut<'a> {
    chunks: std::slice::ChunksExactMut<'a, u8>,
}

impl<'a> Iterator for Rgba8PixelsMut<'a> {
    type Item = Rgba8PixelMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next()?;
        let pixel = Rgba8Pixel {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        };
        Some(Rgba8PixelMut { pixel, bytes })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl<'a> DoubleEndedIterator for Rgba8PixelsMut<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next_back()?;
        let pixel = Rgba8Pixel {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        };
        Some(Rgba8PixelMut { pixel, bytes })
    }
}

impl ExactSizeIterator for Rgba8PixelsMut<'_> {}
impl std::iter::FusedIterator for Rgba8PixelsMut<'_> {}

/// Immutable iterator over typed Gray8 pixels.
#[derive(Clone, Debug)]
pub struct Gray8Pixels<'a> {
    values: std::slice::Iter<'a, u8>,
}

impl Iterator for Gray8Pixels<'_> {
    type Item = Gray8Pixel;

    fn next(&mut self) -> Option<Self::Item> {
        self.values.next().map(|&value| Gray8Pixel { value })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.values.size_hint()
    }
}

impl DoubleEndedIterator for Gray8Pixels<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.values.next_back().map(|&value| Gray8Pixel { value })
    }
}

impl ExactSizeIterator for Gray8Pixels<'_> {}
impl std::iter::FusedIterator for Gray8Pixels<'_> {}

/// Mutable proxy for one Gray8 pixel.
///
/// Field edits are copied back to the image when this value is dropped. The
/// proxy dereferences to [`Gray8Pixel`].
#[derive(Debug)]
pub struct Gray8PixelMut<'a> {
    pixel: Gray8Pixel,
    value: &'a mut u8,
}

impl std::ops::Deref for Gray8PixelMut<'_> {
    type Target = Gray8Pixel;

    fn deref(&self) -> &Self::Target {
        &self.pixel
    }
}

impl std::ops::DerefMut for Gray8PixelMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pixel
    }
}

impl Drop for Gray8PixelMut<'_> {
    fn drop(&mut self) {
        *self.value = self.pixel.value;
    }
}

/// Mutable iterator over typed Gray8 pixels.
#[derive(Debug)]
pub struct Gray8PixelsMut<'a> {
    values: std::slice::IterMut<'a, u8>,
}

impl<'a> Iterator for Gray8PixelsMut<'a> {
    type Item = Gray8PixelMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.values.next().map(|value| Gray8PixelMut {
            pixel: Gray8Pixel { value: *value },
            value,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.values.size_hint()
    }
}

impl<'a> DoubleEndedIterator for Gray8PixelsMut<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.values.next_back().map(|value| Gray8PixelMut {
            pixel: Gray8Pixel { value: *value },
            value,
        })
    }
}

impl ExactSizeIterator for Gray8PixelsMut<'_> {}
impl std::iter::FusedIterator for Gray8PixelsMut<'_> {}

/// Immutable iterator over typed GrayAlpha8 pixels.
#[derive(Clone, Debug)]
pub struct GrayAlpha8Pixels<'a> {
    chunks: std::slice::ChunksExact<'a, u8>,
}

impl Iterator for GrayAlpha8Pixels<'_> {
    type Item = GrayAlpha8Pixel;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next()?;
        Some(GrayAlpha8Pixel {
            value: bytes[0],
            alpha: bytes[1],
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl DoubleEndedIterator for GrayAlpha8Pixels<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next_back()?;
        Some(GrayAlpha8Pixel {
            value: bytes[0],
            alpha: bytes[1],
        })
    }
}

impl ExactSizeIterator for GrayAlpha8Pixels<'_> {}
impl std::iter::FusedIterator for GrayAlpha8Pixels<'_> {}

/// Mutable proxy for one GrayAlpha8 pixel.
///
/// Field edits are copied back to the image when this value is dropped. The
/// proxy dereferences to [`GrayAlpha8Pixel`].
#[derive(Debug)]
pub struct GrayAlpha8PixelMut<'a> {
    pixel: GrayAlpha8Pixel,
    bytes: &'a mut [u8],
}

impl std::ops::Deref for GrayAlpha8PixelMut<'_> {
    type Target = GrayAlpha8Pixel;

    fn deref(&self) -> &Self::Target {
        &self.pixel
    }
}

impl std::ops::DerefMut for GrayAlpha8PixelMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pixel
    }
}

impl Drop for GrayAlpha8PixelMut<'_> {
    fn drop(&mut self) {
        self.bytes
            .copy_from_slice(&[self.pixel.value, self.pixel.alpha]);
    }
}

/// Mutable iterator over typed GrayAlpha8 pixels.
#[derive(Debug)]
pub struct GrayAlpha8PixelsMut<'a> {
    chunks: std::slice::ChunksExactMut<'a, u8>,
}

impl<'a> Iterator for GrayAlpha8PixelsMut<'a> {
    type Item = GrayAlpha8PixelMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next()?;
        let pixel = GrayAlpha8Pixel {
            value: bytes[0],
            alpha: bytes[1],
        };
        Some(GrayAlpha8PixelMut { pixel, bytes })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl<'a> DoubleEndedIterator for GrayAlpha8PixelsMut<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let bytes = self.chunks.next_back()?;
        let pixel = GrayAlpha8Pixel {
            value: bytes[0],
            alpha: bytes[1],
        };
        Some(GrayAlpha8PixelMut { pixel, bytes })
    }
}

impl ExactSizeIterator for GrayAlpha8PixelsMut<'_> {}
impl std::iter::FusedIterator for GrayAlpha8PixelsMut<'_> {}

/// One typed raster pixel from a format-independent iterator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RasterPixel {
    /// RGBA8 pixel.
    Rgba8(Rgba8Pixel),
    /// Single-channel Gray8 pixel.
    Gray8(Gray8Pixel),
    /// GrayAlpha8 pixel.
    GrayAlpha8(GrayAlpha8Pixel),
}

impl RasterPixel {
    /// Pixel format represented by this value.
    #[must_use]
    pub const fn format(self) -> PixelFormat {
        match self {
            Self::Rgba8(_) => PixelFormat::Rgba8,
            Self::Gray8(_) => PixelFormat::Gray8,
            Self::GrayAlpha8(_) => PixelFormat::GrayAlpha8,
        }
    }
}

/// One mutable pixel proxy from a format-independent iterator.
#[derive(Debug)]
#[non_exhaustive]
pub enum RasterPixelMut<'a> {
    /// RGBA8 pixel.
    Rgba8(Rgba8PixelMut<'a>),
    /// Single-channel Gray8 pixel.
    Gray8(Gray8PixelMut<'a>),
    /// GrayAlpha8 pixel.
    GrayAlpha8(GrayAlpha8PixelMut<'a>),
}

impl RasterPixelMut<'_> {
    /// Pixel format represented by this proxy.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        match self {
            Self::Rgba8(_) => PixelFormat::Rgba8,
            Self::Gray8(_) => PixelFormat::Gray8,
            Self::GrayAlpha8(_) => PixelFormat::GrayAlpha8,
        }
    }

    /// Inverts color/value channels while preserving independent alpha.
    pub fn invert(&mut self) {
        match self {
            Self::Rgba8(pixel) => pixel.invert(),
            Self::Gray8(pixel) => pixel.invert(),
            Self::GrayAlpha8(pixel) => pixel.invert(),
        }
    }

    /// Adds to color/value channels if every result is representable.
    ///
    /// Returns `false` and leaves the pixel unchanged if any channel would
    /// overflow. Independent alpha is always preserved.
    pub fn checked_add_assign(&mut self, value: u8) -> bool {
        match self {
            Self::Rgba8(pixel) => {
                let Some(next) = pixel.checked_add_rgb(value) else {
                    return false;
                };
                **pixel = next;
            }
            Self::Gray8(pixel) => {
                let Some(next) = pixel.checked_add(value) else {
                    return false;
                };
                **pixel = next;
            }
            Self::GrayAlpha8(pixel) => {
                let Some(next) = pixel.checked_add_value(value) else {
                    return false;
                };
                **pixel = next;
            }
        }
        true
    }

    /// Subtracts from color/value channels if every result is representable.
    ///
    /// Returns `false` and leaves the pixel unchanged if any channel would
    /// underflow. Independent alpha is always preserved.
    pub fn checked_sub_assign(&mut self, value: u8) -> bool {
        match self {
            Self::Rgba8(pixel) => {
                let Some(next) = pixel.checked_sub_rgb(value) else {
                    return false;
                };
                **pixel = next;
            }
            Self::Gray8(pixel) => {
                let Some(next) = pixel.checked_sub(value) else {
                    return false;
                };
                **pixel = next;
            }
            Self::GrayAlpha8(pixel) => {
                let Some(next) = pixel.checked_sub_value(value) else {
                    return false;
                };
                **pixel = next;
            }
        }
        true
    }

    /// Adds to color/value channels and clamps every result to its range.
    ///
    /// Independent alpha is preserved.
    pub fn saturating_add_assign(&mut self, value: u8) {
        match self {
            Self::Rgba8(pixel) => **pixel = pixel.saturating_add_rgb(value),
            Self::Gray8(pixel) => **pixel = pixel.saturating_add(value),
            Self::GrayAlpha8(pixel) => **pixel = pixel.saturating_add_value(value),
        }
    }

    /// Subtracts from color/value channels and clamps every result to its range.
    ///
    /// Independent alpha is preserved.
    pub fn saturating_sub_assign(&mut self, value: u8) {
        match self {
            Self::Rgba8(pixel) => **pixel = pixel.saturating_sub_rgb(value),
            Self::Gray8(pixel) => **pixel = pixel.saturating_sub(value),
            Self::GrayAlpha8(pixel) => **pixel = pixel.saturating_sub_value(value),
        }
    }
}

/// Format-independent immutable raster-pixel iterator.
#[derive(Clone, Debug)]
pub enum RasterPixels<'a> {
    /// RGBA8 pixels.
    Rgba8(Rgba8Pixels<'a>),
    /// Single-channel Gray8 pixels.
    Gray8(Gray8Pixels<'a>),
    /// GrayAlpha8 pixels.
    GrayAlpha8(GrayAlpha8Pixels<'a>),
}

impl Iterator for RasterPixels<'_> {
    type Item = RasterPixel;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Rgba8(pixels) => pixels.next().map(RasterPixel::Rgba8),
            Self::Gray8(pixels) => pixels.next().map(RasterPixel::Gray8),
            Self::GrayAlpha8(pixels) => pixels.next().map(RasterPixel::GrayAlpha8),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Rgba8(pixels) => pixels.size_hint(),
            Self::Gray8(pixels) => pixels.size_hint(),
            Self::GrayAlpha8(pixels) => pixels.size_hint(),
        }
    }
}

impl DoubleEndedIterator for RasterPixels<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Rgba8(pixels) => pixels.next_back().map(RasterPixel::Rgba8),
            Self::Gray8(pixels) => pixels.next_back().map(RasterPixel::Gray8),
            Self::GrayAlpha8(pixels) => pixels.next_back().map(RasterPixel::GrayAlpha8),
        }
    }
}

impl ExactSizeIterator for RasterPixels<'_> {}
impl std::iter::FusedIterator for RasterPixels<'_> {}

/// Format-independent mutable raster-pixel iterator.
#[derive(Debug)]
pub enum RasterPixelsMut<'a> {
    /// RGBA8 pixels.
    Rgba8(Rgba8PixelsMut<'a>),
    /// Single-channel Gray8 pixels.
    Gray8(Gray8PixelsMut<'a>),
    /// GrayAlpha8 pixels.
    GrayAlpha8(GrayAlpha8PixelsMut<'a>),
}

impl<'a> Iterator for RasterPixelsMut<'a> {
    type Item = RasterPixelMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Rgba8(pixels) => pixels.next().map(RasterPixelMut::Rgba8),
            Self::Gray8(pixels) => pixels.next().map(RasterPixelMut::Gray8),
            Self::GrayAlpha8(pixels) => pixels.next().map(RasterPixelMut::GrayAlpha8),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Rgba8(pixels) => pixels.size_hint(),
            Self::Gray8(pixels) => pixels.size_hint(),
            Self::GrayAlpha8(pixels) => pixels.size_hint(),
        }
    }
}

impl<'a> DoubleEndedIterator for RasterPixelsMut<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Rgba8(pixels) => pixels.next_back().map(RasterPixelMut::Rgba8),
            Self::Gray8(pixels) => pixels.next_back().map(RasterPixelMut::Gray8),
            Self::GrayAlpha8(pixels) => pixels.next_back().map(RasterPixelMut::GrayAlpha8),
        }
    }
}

impl ExactSizeIterator for RasterPixelsMut<'_> {}
impl std::iter::FusedIterator for RasterPixelsMut<'_> {}

/// Origin and external-data resolution state of raster pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RasterDataState {
    /// Pixels were constructed by the caller rather than decoded from a file.
    Constructed,
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

    /// Whether the pixels were constructed by the caller.
    #[must_use]
    pub const fn is_constructed(self) -> bool {
        matches!(self, Self::Constructed)
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
    /// Constructs a validated row-major raster image.
    ///
    /// The byte length must equal `width * height * format bytes-per-pixel`.
    /// Zero dimensions and arithmetic overflow are rejected.
    pub fn from_pixels(
        width: u32,
        height: u32,
        format: PixelFormat,
        pixels: impl Into<Vec<u8>>,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidRaster {
                reason: "constructed raster dimensions must be non-zero".to_owned(),
            });
        }
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|count| count.checked_mul(format.bytes_per_pixel()))
            .ok_or(Error::OffsetOverflow)?;
        let pixels = pixels.into();
        if pixels.len() as u64 != expected {
            return Err(Error::InvalidRaster {
                reason: format!(
                    "constructed {format:?} raster has {} bytes, expected {expected}",
                    pixels.len()
                ),
            });
        }
        Ok(Self {
            width,
            height,
            format,
            state: RasterDataState::Constructed,
            pixels,
        })
    }

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

    /// Number of row-major pixel bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    /// Iterates over typed pixels without requiring a format-specific method.
    ///
    /// Match [`RasterPixel`] when format-specific channel fields are needed.
    #[must_use]
    pub fn pixel_iter(&self) -> RasterPixels<'_> {
        match self.format {
            PixelFormat::Rgba8 => RasterPixels::Rgba8(Rgba8Pixels {
                chunks: self.pixels.chunks_exact(4),
            }),
            PixelFormat::Gray8 => RasterPixels::Gray8(Gray8Pixels {
                values: self.pixels.iter(),
            }),
            PixelFormat::GrayAlpha8 => RasterPixels::GrayAlpha8(GrayAlpha8Pixels {
                chunks: self.pixels.chunks_exact(2),
            }),
        }
    }

    /// Mutably iterates over typed pixels without format-specific branching.
    ///
    /// Common operations such as [`RasterPixelMut::invert`],
    /// [`RasterPixelMut::checked_add_assign`], and
    /// [`RasterPixelMut::saturating_sub_assign`] work uniformly for every
    /// supported format while preserving independent alpha. Match
    /// [`RasterPixelMut`] or use the format-specific iterators when direct
    /// channel fields are needed.
    pub fn pixel_iter_mut(&mut self) -> RasterPixelsMut<'_> {
        match self.format {
            PixelFormat::Rgba8 => RasterPixelsMut::Rgba8(Rgba8PixelsMut {
                chunks: self.pixels.chunks_exact_mut(4),
            }),
            PixelFormat::Gray8 => RasterPixelsMut::Gray8(Gray8PixelsMut {
                values: self.pixels.iter_mut(),
            }),
            PixelFormat::GrayAlpha8 => RasterPixelsMut::GrayAlpha8(GrayAlpha8PixelsMut {
                chunks: self.pixels.chunks_exact_mut(2),
            }),
        }
    }

    /// Typed RGBA8 pixels, or `None` when this image has another format.
    #[must_use]
    pub fn rgba8_pixels(&self) -> Option<Rgba8Pixels<'_>> {
        if self.format == PixelFormat::Rgba8 {
            Some(Rgba8Pixels {
                chunks: self.pixels.chunks_exact(4),
            })
        } else {
            None
        }
    }

    /// Typed mutable RGBA8 pixels, or `None` for another format.
    ///
    /// Each yielded proxy exposes `r`, `g`, `b`, and `a` fields and writes
    /// field changes back before the next loop iteration.
    pub fn rgba8_pixels_mut(&mut self) -> Option<Rgba8PixelsMut<'_>> {
        if self.format == PixelFormat::Rgba8 {
            Some(Rgba8PixelsMut {
                chunks: self.pixels.chunks_exact_mut(4),
            })
        } else {
            None
        }
    }

    /// Typed Gray8 pixels, or `None` when this image has another format.
    #[must_use]
    pub fn gray8_pixels(&self) -> Option<Gray8Pixels<'_>> {
        if self.format == PixelFormat::Gray8 {
            Some(Gray8Pixels {
                values: self.pixels.iter(),
            })
        } else {
            None
        }
    }

    /// Typed mutable Gray8 pixels, or `None` for another format.
    ///
    /// Gray8 has one `value` channel and no independent alpha channel.
    pub fn gray8_pixels_mut(&mut self) -> Option<Gray8PixelsMut<'_>> {
        if self.format == PixelFormat::Gray8 {
            Some(Gray8PixelsMut {
                values: self.pixels.iter_mut(),
            })
        } else {
            None
        }
    }

    /// Typed GrayAlpha8 pixels, or `None` when this image has another format.
    #[must_use]
    pub fn gray_alpha8_pixels(&self) -> Option<GrayAlpha8Pixels<'_>> {
        if self.format == PixelFormat::GrayAlpha8 {
            Some(GrayAlpha8Pixels {
                chunks: self.pixels.chunks_exact(2),
            })
        } else {
            None
        }
    }

    /// Typed mutable GrayAlpha8 pixels, or `None` for another format.
    ///
    /// Each yielded proxy exposes independent `value` and `alpha` fields.
    pub fn gray_alpha8_pixels_mut(&mut self) -> Option<GrayAlpha8PixelsMut<'_>> {
        if self.format == PixelFormat::GrayAlpha8 {
            Some(GrayAlpha8PixelsMut {
                chunks: self.pixels.chunks_exact_mut(2),
            })
        } else {
            None
        }
    }

    /// Converts this raster to an image-rs dynamic image without copying pixels.
    ///
    /// This is available with the optional `image` feature. CLIP-specific
    /// metadata such as [`Self::data_state`] is not represented by
    /// [`image::DynamicImage`], so inspect it before consuming this value when
    /// it matters to the caller.
    #[cfg(feature = "image")]
    #[must_use]
    pub fn into_dynamic_image(self) -> image::DynamicImage {
        let Self {
            width,
            height,
            format,
            pixels,
            ..
        } = self;
        match format {
            PixelFormat::Rgba8 => image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, pixels)
                    .expect("RasterImage maintains its RGBA8 buffer length invariant"),
            ),
            PixelFormat::Gray8 => image::DynamicImage::ImageLuma8(
                image::GrayImage::from_raw(width, height, pixels)
                    .expect("RasterImage maintains its Gray8 buffer length invariant"),
            ),
            PixelFormat::GrayAlpha8 => image::DynamicImage::ImageLumaA8(
                image::GrayAlphaImage::from_raw(width, height, pixels)
                    .expect("RasterImage maintains its GrayAlpha8 buffer length invariant"),
            ),
        }
    }

    /// Converts a supported image-rs dynamic image into a semantic raster.
    ///
    /// Eight-bit RGBA, grayscale, and grayscale-alpha buffers are moved
    /// without copying. Eight-bit RGB is expanded with opaque alpha. Higher
    /// bit depths and floating-point variants are rejected rather than
    /// silently quantized.
    #[cfg(feature = "image")]
    pub fn try_from_dynamic_image(image: image::DynamicImage) -> Result<Self> {
        match image {
            image::DynamicImage::ImageRgba8(image) => Self::from_pixels(
                image.width(),
                image.height(),
                PixelFormat::Rgba8,
                image.into_raw(),
            ),
            image::DynamicImage::ImageRgb8(image) => {
                let width = image.width();
                let height = image.height();
                Self::from_pixels(
                    width,
                    height,
                    PixelFormat::Rgba8,
                    image::DynamicImage::ImageRgb8(image).into_rgba8().into_raw(),
                )
            }
            image::DynamicImage::ImageLuma8(image) => Self::from_pixels(
                image.width(),
                image.height(),
                PixelFormat::Gray8,
                image.into_raw(),
            ),
            image::DynamicImage::ImageLumaA8(image) => Self::from_pixels(
                image.width(),
                image.height(),
                PixelFormat::GrayAlpha8,
                image.into_raw(),
            ),
            _ => Err(Error::UnsupportedRaster {
                reason: "image-rs conversion supports only 8-bit RGB(A), grayscale, and grayscale-alpha images"
                    .to_owned(),
            }),
        }
    }

    /// Takes ownership of the row-major pixels.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
pub(crate) struct RasterEncoder<'a> {
    attributes: &'a OffscreenAttributes,
    format: PixelFormat,
    pixels: &'a [u8],
    tile_bytes: usize,
    tile_count: u32,
}

#[cfg(all(feature = "write", feature = "raster"))]
impl<'a> RasterEncoder<'a> {
    pub(crate) fn new(
        attributes: &'a OffscreenAttributes,
        format: PixelFormat,
        pixels: &'a [u8],
        dimension_limit: u32,
        raster_limit: u64,
        tile_limit: u64,
    ) -> Result<Self> {
        validate_dimensions(attributes, dimension_limit)?;
        let expected_format = pixel_format(attributes.packing())?;
        if format != expected_format {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "replacement pixel format {format:?} does not match raster format {expected_format:?}"
                ),
            });
        }
        let expected_pixels = u64::from(attributes.bitmap_width())
            .checked_mul(u64::from(attributes.bitmap_height()))
            .and_then(|value| value.checked_mul(format.bytes_per_pixel()))
            .ok_or(Error::OffsetOverflow)?;
        if expected_pixels > raster_limit {
            return Err(Error::LimitExceeded {
                resource: "replacement raster bytes",
                value: expected_pixels,
                limit: raster_limit,
            });
        }
        if pixels.len() as u64 != expected_pixels {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "replacement raster has {} bytes, expected {expected_pixels}",
                    pixels.len()
                ),
            });
        }
        let packing = attributes.packing();
        let tile_bytes = u64::from(packing.total_channels())
            .checked_mul(u64::from(packing.block_width()))
            .and_then(|value| value.checked_mul(u64::from(packing.block_height())))
            .ok_or(Error::OffsetOverflow)?;
        if tile_bytes > tile_limit {
            return Err(Error::LimitExceeded {
                resource: "replacement raster tile bytes",
                value: tile_bytes,
                limit: tile_limit,
            });
        }
        let tile_count = attributes
            .block_grid_width()
            .checked_mul(attributes.block_grid_height())
            .ok_or(Error::OffsetOverflow)?;
        Ok(Self {
            attributes,
            format,
            pixels,
            tile_bytes: usize::try_from(tile_bytes).map_err(|_| Error::OffsetOverflow)?,
            tile_count,
        })
    }

    pub(crate) const fn tile_count(&self) -> u32 {
        self.tile_count
    }

    pub(crate) fn default_tile(&self) -> Vec<u8> {
        let fill = if self.attributes.default_fill() == 0 {
            0
        } else {
            u8::MAX
        };
        let mut output = vec![fill; self.tile_bytes];
        if self.format == PixelFormat::Rgba8 {
            let tile_area = self.tile_bytes / self.attributes.packing().total_channels() as usize;
            for pixel in output[tile_area..].chunks_exact_mut(4) {
                // Native RGBA tiles use alpha + interleaved B/G/R/X. X is
                // reserved padding, observed as zero in generated documents.
                pixel[3] = 0;
            }
        }
        output
    }

    pub(crate) fn encode_tile(
        &self,
        tile_index: u32,
        original: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        if tile_index >= self.tile_count {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "replacement raster tile index {tile_index} is outside 0..{}",
                    self.tile_count
                ),
            });
        }
        let mut output = match original {
            Some(bytes) if bytes.len() == self.tile_bytes => bytes,
            Some(bytes) => {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "existing raster tile has {} decoded bytes, expected {}",
                        bytes.len(),
                        self.tile_bytes
                    ),
                });
            }
            None => self.default_tile(),
        };
        let packing = self.attributes.packing();
        let tile_x = tile_index % self.attributes.block_grid_width();
        let tile_y = tile_index / self.attributes.block_grid_width();
        let origin_x = tile_x
            .checked_mul(packing.block_width())
            .ok_or(Error::OffsetOverflow)?;
        let origin_y = tile_y
            .checked_mul(packing.block_height())
            .ok_or(Error::OffsetOverflow)?;
        let copy_width = packing
            .block_width()
            .min(self.attributes.bitmap_width().saturating_sub(origin_x));
        let copy_height = packing
            .block_height()
            .min(self.attributes.bitmap_height().saturating_sub(origin_y));
        let tile_area = u64::from(packing.block_width())
            .checked_mul(u64::from(packing.block_height()))
            .ok_or(Error::OffsetOverflow)?;

        for y in 0..copy_height {
            for x in 0..copy_width {
                let target_pixel = u64::from(origin_y + y)
                    .checked_mul(u64::from(self.attributes.bitmap_width()))
                    .and_then(|value| value.checked_add(u64::from(origin_x + x)))
                    .ok_or(Error::OffsetOverflow)?;
                let tile_pixel = u64::from(y)
                    .checked_mul(u64::from(packing.block_width()))
                    .and_then(|value| value.checked_add(u64::from(x)))
                    .ok_or(Error::OffsetOverflow)?;
                match self.format {
                    PixelFormat::Rgba8 => {
                        let source = usize::try_from(
                            target_pixel.checked_mul(4).ok_or(Error::OffsetOverflow)?,
                        )
                        .map_err(|_| Error::OffsetOverflow)?;
                        let alpha =
                            usize::try_from(tile_pixel).map_err(|_| Error::OffsetOverflow)?;
                        let buffer = usize::try_from(
                            tile_area
                                .checked_add(
                                    tile_pixel.checked_mul(4).ok_or(Error::OffsetOverflow)?,
                                )
                                .ok_or(Error::OffsetOverflow)?,
                        )
                        .map_err(|_| Error::OffsetOverflow)?;
                        output[alpha] = self.pixels[source + 3];
                        output[buffer] = self.pixels[source + 2];
                        output[buffer + 1] = self.pixels[source + 1];
                        output[buffer + 2] = self.pixels[source];
                    }
                    PixelFormat::Gray8 => {
                        let source =
                            usize::try_from(target_pixel).map_err(|_| Error::OffsetOverflow)?;
                        let target =
                            usize::try_from(tile_pixel).map_err(|_| Error::OffsetOverflow)?;
                        output[target] = self.pixels[source];
                    }
                    PixelFormat::GrayAlpha8 => {
                        let source = usize::try_from(
                            target_pixel.checked_mul(2).ok_or(Error::OffsetOverflow)?,
                        )
                        .map_err(|_| Error::OffsetOverflow)?;
                        let alpha =
                            usize::try_from(tile_pixel).map_err(|_| Error::OffsetOverflow)?;
                        let value = usize::try_from(
                            tile_area
                                .checked_add(tile_pixel)
                                .ok_or(Error::OffsetOverflow)?,
                        )
                        .map_err(|_| Error::OffsetOverflow)?;
                        output[alpha] = self.pixels[source + 1];
                        output[value] = self.pixels[source];
                    }
                }
            }
        }
        Ok(output)
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
impl<R: Read + Seek> ClipWriter<'_, R> {
    /// Clones one plain raster layer from a complete semantic image.
    ///
    /// The image's pixel format is retained and the compatible checksum is
    /// selected automatically. See [`Self::clone_raster_layer_from_template`]
    /// for raw row-major interoperability.
    pub fn clone_raster_layer_from_template_image(
        &mut self,
        template_layer_id: i64,
        parent_layer_id: i64,
        layer_name: impl AsRef<str>,
        image: RasterImage,
        limits: Limits,
    ) -> Result<i64> {
        let format = image.format();
        self.clone_raster_layer_from_template(
            template_layer_id,
            parent_layer_id,
            layer_name,
            format,
            image.into_pixels(),
            limits,
        )
    }

    /// Clones one plain raster layer and replaces its base pixels.
    ///
    /// The template and destination parent must belong to the same canvas.
    /// Every unknown column in the template's `Layer`, `Mipmap`,
    /// `MipmapInfo`, `Offscreen`, and `LayerThumbnail` rows is preserved.
    /// Row identities, semantic IDs, the layer UUID, external identifiers,
    /// ownership references, and tree links are regenerated transactionally.
    ///
    /// Only the 100% base mipmap receives a new external block-data body.
    /// Its populated tiles use the CLIP STUDIO PAINT-compatible checksum.
    /// Observed CLIP files store lower mipmap levels as cache references whose
    /// identifiers are absent from `ExternalChunk`; this method gives every
    /// derived level and the thumbnail atlas a fresh absent identifier so no
    /// pixels from the template can be displayed as a stale cache.
    ///
    /// The new layer is inserted as the first child of `parent_layer_id`.
    /// The template must be a leaf with `LayerType = 1`, no layer mask, one
    /// valid render-mipmap chain, and one valid render-thumbnail row.
    pub fn clone_raster_layer_from_template(
        &mut self,
        template_layer_id: i64,
        parent_layer_id: i64,
        layer_name: impl AsRef<str>,
        format: PixelFormat,
        pixels: impl AsRef<[u8]>,
        limits: Limits,
    ) -> Result<i64> {
        let layer_name = layer_name.as_ref();
        enforce_raster_clone_limit(
            layer_name.len() as u64,
            limits.max_text_bytes(),
            "new raster layer name bytes",
        )?;
        let template = RasterLayerTemplate::read(
            self.database().connection(),
            template_layer_id,
            parent_layer_id,
            limits,
        )?;
        let encoder = RasterEncoder::new(
            &template.levels[0].attributes,
            format,
            pixels.as_ref(),
            limits.max_canvas_dimension(),
            limits.max_raster_bytes(),
            limits.max_decompressed_block_size(),
        )?;
        let source_body = self.external_body_for_update(
            &template.base_identifier,
            limits.max_write_external_body_size(),
        )?;
        let mut replacements = BTreeMap::new();
        for tile_index in 0..encoder.tile_count() {
            replacements.insert(tile_index, encoder.encode_tile(tile_index, None)?);
        }
        let rebuilt = rebuild_block_data_body_batch(
            &source_body,
            &replacements,
            BlockChecksumPolicy::CspCompatible,
            limits.max_blocks_per_external(),
            limits.max_decompressed_block_size(),
            limits.max_write_external_body_size(),
        )?;
        if rebuilt.blocks.len() != usize::try_from(encoder.tile_count()).unwrap_or(usize::MAX) {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "rebuilt raster contains {} tiles, expected {}",
                    rebuilt.blocks.len(),
                    encoder.tile_count()
                ),
            });
        }
        let base_attribute = replace_attribute_block_sizes(
            &template.levels[0].attribute,
            &rebuilt
                .blocks
                .iter()
                .map(|block| block.block_record_size)
                .collect::<Vec<_>>(),
        )?;

        let generated_identifier_count = template
            .levels
            .len()
            .checked_add(1)
            .ok_or(Error::OffsetOverflow)?;
        let identifiers = generate_raster_external_identifiers(
            self.database().connection(),
            generated_identifier_count,
            limits,
        )?;
        let base_identifier = identifiers[0].clone();
        self.add_external_body(&base_identifier, rebuilt.body)?;

        let insertion = RasterLayerCloneInsertion::prepare(
            self.database().connection(),
            &template,
            layer_name,
            base_attribute,
            identifiers,
            limits,
        );
        let result = insertion.and_then(|insertion| {
            insert_raster_layer_clone(self.database_mut().connection_mut(), insertion)
        });
        match result {
            Ok(layer_id) => Ok(layer_id),
            Err(error) => {
                if self.unstage_new_external_body(&base_identifier).is_none() {
                    return Err(Error::InvalidWrite {
                        reason:
                            "raster clone failed and its staged external body could not be rolled back"
                                .to_owned(),
                    });
                }
                Err(error)
            }
        }
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
#[derive(Clone, Copy)]
enum RasterBlockDataStorage {
    Text,
    Blob,
}

#[cfg(all(feature = "write", feature = "raster"))]
impl RasterBlockDataStorage {
    fn value(self, identifier: &[u8]) -> Result<Value> {
        match self {
            Self::Text => Ok(Value::Text(
                String::from_utf8(identifier.to_vec()).map_err(|_| Error::InvalidWrite {
                    reason: "generated raster external identifier is not valid UTF-8".to_owned(),
                })?,
            )),
            Self::Blob => Ok(Value::Blob(identifier.to_vec())),
        }
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
struct RasterMipmapTemplate {
    info_id: i64,
    offscreen_id: i64,
    attribute: Vec<u8>,
    attributes: OffscreenAttributes,
    block_data_storage: RasterBlockDataStorage,
}

#[cfg(all(feature = "write", feature = "raster"))]
struct RasterLayerTemplate {
    layer_id: i64,
    canvas_id: i64,
    parent_layer_id: i64,
    parent_first_child: i64,
    mipmap_id: i64,
    thumbnail_id: i64,
    thumbnail_offscreen_id: i64,
    thumbnail_block_data_storage: RasterBlockDataStorage,
    levels: Vec<RasterMipmapTemplate>,
    base_identifier: Vec<u8>,
}

#[cfg(all(feature = "write", feature = "raster"))]
impl RasterLayerTemplate {
    fn read(
        connection: &rusqlite::Connection,
        layer_id: i64,
        parent_layer_id: i64,
        limits: Limits,
    ) -> Result<Self> {
        for (table, columns) in [
            (
                "Layer",
                &[
                    "MainId",
                    "CanvasId",
                    "LayerName",
                    "LayerType",
                    "LayerFolder",
                    "LayerSelect",
                    "LayerNextIndex",
                    "LayerFirstChildIndex",
                    "LayerRenderMipmap",
                    "LayerLayerMaskMipmap",
                    "LayerRenderThumbnail",
                    "LayerLayerMaskThumbnail",
                    "LayerUuid",
                ][..],
            ),
            (
                "Mipmap",
                &[
                    "MainId",
                    "CanvasId",
                    "LayerId",
                    "MipmapCount",
                    "BaseMipmapInfo",
                ][..],
            ),
            (
                "MipmapInfo",
                &[
                    "MainId",
                    "CanvasId",
                    "LayerId",
                    "ThisScale",
                    "Offscreen",
                    "NextIndex",
                ][..],
            ),
            (
                "Offscreen",
                &["MainId", "CanvasId", "LayerId", "Attribute", "BlockData"][..],
            ),
            (
                "LayerThumbnail",
                &["MainId", "CanvasId", "LayerId", "ThumbnailOffscreen"][..],
            ),
            ("Canvas", &["MainId", "CanvasRootFolder"][..]),
            ("ElemScheme", &["TableName", "MaxIndex"][..]),
        ] {
            require_raster_clone_columns(connection, table, columns)?;
            cloneable_table_columns(connection, table)?;
        }
        let layer_count: i64 =
            connection.query_row("SELECT count(*) FROM Layer", [], |row| row.get(0))?;
        let resulting_count = u64::try_from(layer_count)
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or(Error::OffsetOverflow)?;
        enforce_raster_clone_limit(
            resulting_count,
            limits.max_layers(),
            "layers after raster clone",
        )?;
        let parent_depth = validate_raster_canvas_tree(connection, parent_layer_id, limits)?;
        let new_layer_depth = parent_depth.checked_add(1).ok_or(Error::OffsetOverflow)?;
        enforce_raster_clone_limit(
            new_layer_depth,
            limits.max_layer_tree_depth(),
            "new raster layer tree depth",
        )?;

        let (
            canvas_id,
            layer_type,
            layer_folder,
            first_child,
            mipmap_id,
            mask_mipmap_id,
            thumbnail_id,
            mask_thumbnail_id,
        ): (i64, i64, i64, i64, i64, i64, i64, i64) = query_unique_raster_row(
            connection,
            "Layer",
            layer_id,
            "SELECT CanvasId, LayerType, LayerFolder, \
             coalesce(LayerFirstChildIndex, 0), coalesce(LayerRenderMipmap, 0), \
             coalesce(LayerLayerMaskMipmap, 0), coalesce(LayerRenderThumbnail, 0), \
             coalesce(LayerLayerMaskThumbnail, 0) FROM Layer WHERE MainId = ?1",
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )?;
        if layer_type != 1
            || layer_folder != 0
            || first_child != 0
            || mipmap_id <= 0
            || thumbnail_id <= 0
            || mask_mipmap_id != 0
            || mask_thumbnail_id != 0
        {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "raster template layer {layer_id} is not a plain unmasked pixel leaf"
                ),
            });
        }
        let (parent_canvas_id, parent_folder, parent_first_child): (i64, i64, i64) =
            query_unique_raster_row(
                connection,
                "Layer",
                parent_layer_id,
                "SELECT CanvasId, LayerFolder, coalesce(LayerFirstChildIndex, 0) \
                 FROM Layer WHERE MainId = ?1",
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if parent_canvas_id != canvas_id || parent_folder == 0 {
            return Err(Error::InvalidWrite {
                reason: "raster clone parent is not a folder on the template canvas".to_owned(),
            });
        }

        let (mipmap_canvas_id, mipmap_layer_id, mipmap_count, mut current): (i64, i64, i64, i64) =
            query_unique_raster_row(
                connection,
                "Mipmap",
                mipmap_id,
                "SELECT CanvasId, LayerId, MipmapCount, BaseMipmapInfo \
             FROM Mipmap WHERE MainId = ?1",
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        if mipmap_canvas_id != canvas_id || mipmap_layer_id != layer_id || mipmap_count <= 0 {
            return Err(Error::InvalidWrite {
                reason: "raster template Mipmap ownership or count is invalid".to_owned(),
            });
        }
        enforce_raster_clone_limit(
            u64::try_from(mipmap_count).unwrap_or(u64::MAX),
            limits.max_layers(),
            "raster mipmap levels",
        )?;
        let mut seen = BTreeSet::new();
        let mut levels = Vec::new();
        let mut previous_scale = f64::INFINITY;
        let mut base_identifier = None;
        while current != 0 {
            if !seen.insert(current) {
                return Err(Error::InvalidWrite {
                    reason: "raster template MipmapInfo chain contains a cycle".to_owned(),
                });
            }
            let (info_canvas_id, info_layer_id, scale, offscreen_id, next): (
                i64,
                i64,
                f64,
                i64,
                i64,
            ) = query_unique_raster_row(
                connection,
                "MipmapInfo",
                current,
                "SELECT CanvasId, LayerId, ThisScale, Offscreen, coalesce(NextIndex, 0) \
                 FROM MipmapInfo WHERE MainId = ?1",
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
            if info_canvas_id != canvas_id
                || info_layer_id != layer_id
                || !scale.is_finite()
                || scale <= 0.0
                || scale >= previous_scale
                || offscreen_id <= 0
            {
                return Err(Error::InvalidWrite {
                    reason: "raster template MipmapInfo chain metadata is invalid".to_owned(),
                });
            }
            if levels.is_empty() && scale != 100.0 {
                return Err(Error::InvalidWrite {
                    reason: format!("raster template base mipmap scale is {scale}, expected 100"),
                });
            }
            let (offscreen_canvas_id, offscreen_layer_id, attribute, block_data): (
                i64,
                i64,
                Vec<u8>,
                Value,
            ) = query_unique_raster_row(
                connection,
                "Offscreen",
                offscreen_id,
                "SELECT CanvasId, LayerId, Attribute, BlockData \
                 FROM Offscreen WHERE MainId = ?1",
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            if offscreen_canvas_id != canvas_id || offscreen_layer_id != layer_id {
                return Err(Error::InvalidWrite {
                    reason: "raster template Offscreen ownership is invalid".to_owned(),
                });
            }
            let attributes = OffscreenAttributes::parse(&attribute)?;
            let block_data = raster_block_data(block_data)?;
            let block_data_storage = block_data
                .as_ref()
                .map_or(RasterBlockDataStorage::Blob, |(_, storage)| *storage);
            if levels.is_empty() {
                let identifier = block_data
                    .as_ref()
                    .map(|(identifier, _)| identifier)
                    .filter(|identifier| !identifier.is_empty())
                    .ok_or_else(|| Error::InvalidWrite {
                        reason: "raster template base Offscreen has no external identifier"
                            .to_owned(),
                    })?;
                base_identifier = Some(identifier.clone());
            }
            levels.push(RasterMipmapTemplate {
                info_id: current,
                offscreen_id,
                attribute,
                attributes,
                block_data_storage,
            });
            previous_scale = scale;
            current = next;
        }
        if levels.len() != usize::try_from(mipmap_count).unwrap_or(usize::MAX) {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "raster template Mipmap declares {mipmap_count} levels but its chain has {}",
                    levels.len()
                ),
            });
        }
        let base_identifier = base_identifier.ok_or_else(|| Error::InvalidWrite {
            reason: "raster template has no base external identifier".to_owned(),
        })?;
        enforce_raster_clone_limit(
            base_identifier.len() as u64,
            limits.max_identifier_size(),
            "raster base external identifier",
        )?;

        let (thumbnail_canvas_id, thumbnail_layer_id, thumbnail_offscreen_id): (i64, i64, i64) =
            query_unique_raster_row(
                connection,
                "LayerThumbnail",
                thumbnail_id,
                "SELECT CanvasId, LayerId, ThumbnailOffscreen \
                 FROM LayerThumbnail WHERE MainId = ?1",
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if thumbnail_canvas_id != canvas_id
            || thumbnail_layer_id != layer_id
            || thumbnail_offscreen_id <= 0
        {
            return Err(Error::InvalidWrite {
                reason: "raster template LayerThumbnail ownership is invalid".to_owned(),
            });
        }
        let (thumb_canvas_id, thumb_layer_id, thumb_attribute, thumbnail_block_data): (
            i64,
            i64,
            Vec<u8>,
            Value,
        ) = query_unique_raster_row(
            connection,
            "Offscreen",
            thumbnail_offscreen_id,
            "SELECT CanvasId, LayerId, Attribute, BlockData \
                 FROM Offscreen WHERE MainId = ?1",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        if thumb_canvas_id != canvas_id || thumb_layer_id != layer_id {
            return Err(Error::InvalidWrite {
                reason: "raster template thumbnail Offscreen ownership is invalid".to_owned(),
            });
        }
        OffscreenAttributes::parse(&thumb_attribute)?;
        let thumbnail_block_data_storage = raster_block_data(thumbnail_block_data)?
            .map_or(RasterBlockDataStorage::Blob, |(_, storage)| storage);

        Ok(Self {
            layer_id,
            canvas_id,
            parent_layer_id,
            parent_first_child,
            mipmap_id,
            thumbnail_id,
            thumbnail_offscreen_id,
            thumbnail_block_data_storage,
            levels,
            base_identifier,
        })
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
struct RasterLayerCloneInsertion<'a> {
    template: &'a RasterLayerTemplate,
    layer_name: &'a str,
    layer_id: i64,
    layer_uuid: String,
    mipmap_id: i64,
    mipmap_info_ids: Vec<i64>,
    offscreen_ids: Vec<i64>,
    thumbnail_id: i64,
    thumbnail_offscreen_id: i64,
    base_attribute: Vec<u8>,
    identifiers: Vec<Vec<u8>>,
    table_columns: BTreeMap<&'static str, Vec<String>>,
    elem_scheme_maxima: BTreeMap<&'static str, i64>,
}

#[cfg(all(feature = "write", feature = "raster"))]
impl<'a> RasterLayerCloneInsertion<'a> {
    fn prepare(
        connection: &rusqlite::Connection,
        template: &'a RasterLayerTemplate,
        layer_name: &'a str,
        base_attribute: Vec<u8>,
        identifiers: Vec<Vec<u8>>,
        limits: Limits,
    ) -> Result<Self> {
        if identifiers.len() != template.levels.len() + 1 {
            return Err(Error::InvalidWrite {
                reason: "raster clone identifier allocation count is inconsistent".to_owned(),
            });
        }
        let mut table_columns = BTreeMap::new();
        for table in [
            "Layer",
            "Mipmap",
            "MipmapInfo",
            "Offscreen",
            "LayerThumbnail",
        ] {
            table_columns.insert(table, cloneable_table_columns(connection, table)?);
        }
        let (layer_id, layer_max) = allocate_raster_ids(connection, "Layer", 1)?;
        let (mipmap_id, mipmap_max) = allocate_raster_ids(connection, "Mipmap", 1)?;
        let (mipmap_info_start, mipmap_info_max) =
            allocate_raster_ids(connection, "MipmapInfo", template.levels.len())?;
        let offscreen_count = template
            .levels
            .len()
            .checked_add(1)
            .ok_or(Error::OffsetOverflow)?;
        let (offscreen_start, offscreen_max) =
            allocate_raster_ids(connection, "Offscreen", offscreen_count)?;
        let (thumbnail_id, thumbnail_max) = allocate_raster_ids(connection, "LayerThumbnail", 1)?;
        let mipmap_info_ids = sequential_raster_ids(mipmap_info_start, template.levels.len())?;
        let offscreen_ids = sequential_raster_ids(offscreen_start, template.levels.len())?;
        let thumbnail_offscreen_id = offscreen_start
            .checked_add(i64::try_from(template.levels.len()).map_err(|_| Error::OffsetOverflow)?)
            .ok_or(Error::OffsetOverflow)?;
        let layer_uuid = generate_raster_layer_uuid(connection, limits)?;
        Ok(Self {
            template,
            layer_name,
            layer_id,
            layer_uuid,
            mipmap_id,
            mipmap_info_ids,
            offscreen_ids,
            thumbnail_id,
            thumbnail_offscreen_id,
            base_attribute,
            identifiers,
            table_columns,
            elem_scheme_maxima: BTreeMap::from([
                ("Layer", layer_max),
                ("Mipmap", mipmap_max),
                ("MipmapInfo", mipmap_info_max),
                ("Offscreen", offscreen_max),
                ("LayerThumbnail", thumbnail_max),
            ]),
        })
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
fn insert_raster_layer_clone(
    connection: &mut rusqlite::Connection,
    insertion: RasterLayerCloneInsertion<'_>,
) -> Result<i64> {
    let transaction = connection.transaction()?;

    let mut replacements = BTreeMap::from([
        ("MainId", Value::Integer(insertion.layer_id)),
        ("LayerName", Value::Text(insertion.layer_name.to_owned())),
        ("LayerSelect", Value::Integer(0)),
        (
            "LayerNextIndex",
            Value::Integer(insertion.template.parent_first_child),
        ),
        ("LayerFirstChildIndex", Value::Integer(0)),
        ("LayerUuid", Value::Text(insertion.layer_uuid)),
        ("LayerRenderMipmap", Value::Integer(insertion.mipmap_id)),
        ("LayerLayerMaskMipmap", Value::Integer(0)),
        (
            "LayerRenderThumbnail",
            Value::Integer(insertion.thumbnail_id),
        ),
        ("LayerLayerMaskThumbnail", Value::Integer(0)),
    ]);
    insert_raster_clone_row(
        &transaction,
        "Layer",
        &insertion.table_columns["Layer"],
        insertion.template.layer_id,
        &replacements,
    )?;
    replacements.clear();

    let first_info_id = insertion.mipmap_info_ids[0];
    replacements.extend([
        ("MainId", Value::Integer(insertion.mipmap_id)),
        ("LayerId", Value::Integer(insertion.layer_id)),
        ("BaseMipmapInfo", Value::Integer(first_info_id)),
    ]);
    insert_raster_clone_row(
        &transaction,
        "Mipmap",
        &insertion.table_columns["Mipmap"],
        insertion.template.mipmap_id,
        &replacements,
    )?;

    for (index, template_level) in insertion.template.levels.iter().enumerate() {
        let next = insertion
            .mipmap_info_ids
            .get(index + 1)
            .copied()
            .unwrap_or(0);
        let info_replacements = BTreeMap::from([
            ("MainId", Value::Integer(insertion.mipmap_info_ids[index])),
            ("LayerId", Value::Integer(insertion.layer_id)),
            ("Offscreen", Value::Integer(insertion.offscreen_ids[index])),
            ("NextIndex", Value::Integer(next)),
        ]);
        insert_raster_clone_row(
            &transaction,
            "MipmapInfo",
            &insertion.table_columns["MipmapInfo"],
            template_level.info_id,
            &info_replacements,
        )?;
        let attribute = if index == 0 {
            insertion.base_attribute.clone()
        } else {
            template_level.attribute.clone()
        };
        let block_data = template_level
            .block_data_storage
            .value(&insertion.identifiers[index])?;
        let offscreen_replacements = BTreeMap::from([
            ("MainId", Value::Integer(insertion.offscreen_ids[index])),
            ("LayerId", Value::Integer(insertion.layer_id)),
            ("Attribute", Value::Blob(attribute)),
            ("BlockData", block_data),
        ]);
        insert_raster_clone_row(
            &transaction,
            "Offscreen",
            &insertion.table_columns["Offscreen"],
            template_level.offscreen_id,
            &offscreen_replacements,
        )?;
    }

    let thumbnail_replacements = BTreeMap::from([
        ("MainId", Value::Integer(insertion.thumbnail_id)),
        ("LayerId", Value::Integer(insertion.layer_id)),
        (
            "ThumbnailOffscreen",
            Value::Integer(insertion.thumbnail_offscreen_id),
        ),
    ]);
    insert_raster_clone_row(
        &transaction,
        "LayerThumbnail",
        &insertion.table_columns["LayerThumbnail"],
        insertion.template.thumbnail_id,
        &thumbnail_replacements,
    )?;
    let thumbnail_block_data = insertion.template.thumbnail_block_data_storage.value(
        insertion
            .identifiers
            .last()
            .expect("thumbnail identifier count validated"),
    )?;
    let thumbnail_offscreen_replacements = BTreeMap::from([
        ("MainId", Value::Integer(insertion.thumbnail_offscreen_id)),
        ("LayerId", Value::Integer(insertion.layer_id)),
        ("BlockData", thumbnail_block_data),
    ]);
    insert_raster_clone_row(
        &transaction,
        "Offscreen",
        &insertion.table_columns["Offscreen"],
        insertion.template.thumbnail_offscreen_id,
        &thumbnail_offscreen_replacements,
    )?;

    let updated = transaction.execute(
        "UPDATE Layer SET LayerFirstChildIndex = ?1 \
         WHERE MainId = ?2 AND CanvasId = ?3 \
         AND coalesce(LayerFirstChildIndex, 0) = ?4",
        params![
            insertion.layer_id,
            insertion.template.parent_layer_id,
            insertion.template.canvas_id,
            insertion.template.parent_first_child,
        ],
    )?;
    if updated != 1 {
        return Err(Error::InvalidWrite {
            reason: "raster clone parent tree link changed during insertion".to_owned(),
        });
    }
    for (table, maximum) in &insertion.elem_scheme_maxima {
        let updated = transaction.execute(
            "UPDATE ElemScheme SET MaxIndex = ?1 WHERE TableName = ?2",
            params![maximum, table],
        )?;
        if updated != 1 {
            return Err(Error::InvalidWrite {
                reason: format!("ElemScheme {table} update affected {updated} rows"),
            });
        }
    }
    transaction.commit()?;
    Ok(insertion.layer_id)
}

#[cfg(all(feature = "write", feature = "raster"))]
fn require_raster_clone_columns(
    connection: &rusqlite::Connection,
    table: &str,
    columns: &[&str],
) -> Result<()> {
    let available = cloneable_table_columns(connection, table)?;
    for column in columns {
        if !available.iter().any(|candidate| candidate == column) {
            return Err(Error::InvalidWrite {
                reason: format!("raster clone requires {table}.{column}"),
            });
        }
    }
    Ok(())
}

#[cfg(all(feature = "write", feature = "raster"))]
fn cloneable_table_columns(connection: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let sql = format!("PRAGMA table_xinfo({})", quote_raster_identifier(table));
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?;
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Err(Error::InvalidWrite {
            reason: format!("raster clone requires table {table}"),
        });
    }
    let mut columns = Vec::new();
    let mut regeneratable_primary_key = false;
    for (name, primary_key_position, hidden) in rows {
        if name == "_PW_ID" {
            if primary_key_position == 0 || hidden != 0 {
                return Err(Error::InvalidWrite {
                    reason: format!("{table}._PW_ID is not a regeneratable primary key"),
                });
            }
            regeneratable_primary_key = true;
            continue;
        }
        if primary_key_position != 0 {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "raster clone cannot regenerate unexpected {table} primary-key column {name:?}"
                ),
            });
        }
        if hidden == 0 {
            columns.push(name);
        }
    }
    if !regeneratable_primary_key || columns.is_empty() {
        return Err(Error::InvalidWrite {
            reason: format!("{table} has no safely cloneable row layout"),
        });
    }
    Ok(columns)
}

#[cfg(all(feature = "write", feature = "raster"))]
fn insert_raster_clone_row(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    columns: &[String],
    template_id: i64,
    replacements: &BTreeMap<&str, Value>,
) -> Result<()> {
    let column_list = columns
        .iter()
        .map(|column| quote_raster_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut values = Vec::new();
    let selected = columns
        .iter()
        .map(|column| {
            if let Some(value) = replacements.get(column.as_str()) {
                values.push(value.clone());
                format!("?{}", values.len())
            } else {
                format!("template.{}", quote_raster_identifier(column))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    values.push(Value::Integer(template_id));
    let sql = format!(
        "INSERT INTO {} ({column_list}) SELECT {selected} FROM {} AS template \
         WHERE template.MainId = ?{}",
        quote_raster_identifier(table),
        quote_raster_identifier(table),
        values.len(),
    );
    let inserted = transaction.execute(&sql, params_from_iter(values.iter()))?;
    if inserted != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "raster template {table} row {template_id} clone inserted {inserted} rows"
            ),
        });
    }
    Ok(())
}

#[cfg(all(feature = "write", feature = "raster"))]
fn allocate_raster_ids(
    connection: &rusqlite::Connection,
    table: &'static str,
    amount: usize,
) -> Result<(i64, i64)> {
    if amount == 0 {
        return Err(Error::InvalidWrite {
            reason: format!("raster clone requested no {table} IDs"),
        });
    }
    let sql = format!(
        "SELECT count(*), count(MainId), count(DISTINCT MainId), max(MainId) FROM {}",
        quote_raster_identifier(table)
    );
    let (rows, non_null, distinct, maximum): (i64, i64, i64, Option<i64>) =
        connection.query_row(&sql, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
    if rows != non_null || rows != distinct {
        return Err(Error::InvalidWrite {
            reason: format!("{table}.MainId contains NULL or duplicate values"),
        });
    }
    let maximum = maximum.unwrap_or(0);
    if maximum < 0 {
        return Err(Error::InvalidWrite {
            reason: format!("{table}.MainId maximum is negative"),
        });
    }
    let (scheme_rows, scheme_max): (i64, Option<i64>) = connection.query_row(
        "SELECT count(*), max(MaxIndex) FROM ElemScheme WHERE TableName = ?1",
        [table],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if scheme_rows != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("ElemScheme must contain exactly one {table} row, found {scheme_rows}"),
        });
    }
    let scheme_max = scheme_max.ok_or_else(|| Error::InvalidWrite {
        reason: format!("ElemScheme {table} MaxIndex is NULL"),
    })?;
    if scheme_max < maximum {
        return Err(Error::InvalidWrite {
            reason: format!(
                "ElemScheme {table} MaxIndex {scheme_max} is below MainId maximum {maximum}"
            ),
        });
    }
    let amount = i64::try_from(amount).map_err(|_| Error::OffsetOverflow)?;
    let start = scheme_max.checked_add(1).ok_or(Error::OffsetOverflow)?;
    let resulting_maximum = scheme_max
        .checked_add(amount)
        .ok_or(Error::OffsetOverflow)?;
    if start <= 0 {
        return Err(Error::InvalidWrite {
            reason: format!("could not allocate a positive {table}.MainId"),
        });
    }
    Ok((start, resulting_maximum))
}

#[cfg(all(feature = "write", feature = "raster"))]
fn sequential_raster_ids(start: i64, count: usize) -> Result<Vec<i64>> {
    (0..count)
        .map(|index| {
            start
                .checked_add(i64::try_from(index).map_err(|_| Error::OffsetOverflow)?)
                .ok_or(Error::OffsetOverflow)
        })
        .collect()
}

#[cfg(all(feature = "write", feature = "raster"))]
fn generate_raster_layer_uuid(connection: &rusqlite::Connection, limits: Limits) -> Result<String> {
    let mut occupied = BTreeSet::new();
    let mut statement =
        connection.prepare("SELECT LayerUuid FROM Layer WHERE LayerUuid IS NOT NULL")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut count = 0_u64;
    for row in rows {
        count = count.checked_add(1).ok_or(Error::OffsetOverflow)?;
        enforce_raster_clone_limit(count, limits.max_layers(), "raster layer UUIDs")?;
        let value = row?;
        let normalized = normalize_raster_uuid(&value)?;
        if !occupied.insert(normalized) {
            return Err(Error::InvalidWrite {
                reason: "Layer contains duplicate normalized UUIDs".to_owned(),
            });
        }
    }
    for _ in 0..128 {
        let mut random: Vec<u8> =
            connection.query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
        if random.len() != 16 {
            return Err(Error::InvalidWrite {
                reason: "SQLite returned an invalid raster layer UUID seed".to_owned(),
            });
        }
        random[6] = (random[6] & 0x0f) | 0x40;
        random[8] = (random[8] & 0x3f) | 0x80;
        let normalized = format_raster_uuid_hex(&random);
        if occupied.contains(&normalized) {
            continue;
        }
        let standard = format!(
            "{}-{}-{}-{}-{}",
            &normalized[0..8],
            &normalized[8..12],
            &normalized[12..16],
            &normalized[16..20],
            &normalized[20..32],
        );
        return Ok(format!("{}{}", &standard[34..36], &standard[..34]));
    }
    Err(Error::InvalidWrite {
        reason: "could not generate a unique raster layer UUID".to_owned(),
    })
}

#[cfg(all(feature = "write", feature = "raster"))]
fn normalize_raster_uuid(value: &str) -> Result<String> {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    if normalized.len() != 32 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "LayerUuid has {} hexadecimal digits instead of 32",
                normalized.len()
            ),
        });
    }
    Ok(normalized)
}

#[cfg(all(feature = "write", feature = "raster"))]
fn format_raster_uuid_hex(uuid: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(uuid.len() * 2);
    for byte in uuid {
        write!(output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(all(feature = "write", feature = "raster"))]
fn generate_raster_external_identifiers(
    connection: &rusqlite::Connection,
    count: usize,
    limits: Limits,
) -> Result<Vec<Vec<u8>>> {
    enforce_raster_clone_limit(
        40,
        limits.max_identifier_size(),
        "generated raster external identifier",
    )?;
    let mut occupied = BTreeSet::new();
    for (table, column) in [("ExternalChunk", "ExternalID"), ("Offscreen", "BlockData")] {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} IS NOT NULL",
            quote_raster_identifier(column),
            quote_raster_identifier(table),
            quote_raster_identifier(column),
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut row_count = 0_u64;
        while let Some(row) = rows.next()? {
            row_count = row_count.checked_add(1).ok_or(Error::OffsetOverflow)?;
            let combined_limit = limits
                .max_layers()
                .checked_mul(16)
                .ok_or(Error::OffsetOverflow)?;
            enforce_raster_clone_limit(
                row_count,
                combined_limit,
                "raster external identifier candidates",
            )?;
            let identifier = match row.get_ref(0)? {
                ValueRef::Text(value) | ValueRef::Blob(value) => value.to_vec(),
                _ => {
                    return Err(Error::InvalidWrite {
                        reason: format!("{table}.{column} is neither TEXT nor BLOB"),
                    });
                }
            };
            occupied.insert(identifier);
        }
    }
    let mut identifiers = Vec::new();
    while identifiers.len() < count {
        if identifiers.len().checked_add(128).is_none() {
            return Err(Error::OffsetOverflow);
        }
        let mut generated = false;
        for _ in 0..128 {
            let mut random: Vec<u8> =
                connection.query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
            if random.len() != 16 {
                return Err(Error::InvalidWrite {
                    reason: "SQLite returned an invalid raster external ID seed".to_owned(),
                });
            }
            random[6] = (random[6] & 0x0f) | 0x40;
            random[8] = (random[8] & 0x3f) | 0x80;
            let mut identifier = Vec::with_capacity(40);
            identifier.extend_from_slice(b"extrnlid");
            for byte in random {
                identifier.extend_from_slice(format!("{byte:02X}").as_bytes());
            }
            if occupied.insert(identifier.clone()) {
                identifiers.push(identifier);
                generated = true;
                break;
            }
        }
        if !generated {
            return Err(Error::InvalidWrite {
                reason: "could not generate unique raster external identifiers".to_owned(),
            });
        }
    }
    Ok(identifiers)
}

#[cfg(all(feature = "write", feature = "raster"))]
fn raster_block_data(value: Value) -> Result<Option<(Vec<u8>, RasterBlockDataStorage)>> {
    match value {
        Value::Null => Ok(None),
        Value::Text(value) => Ok(Some((value.into_bytes(), RasterBlockDataStorage::Text))),
        Value::Blob(value) => Ok(Some((value, RasterBlockDataStorage::Blob))),
        _ => Err(Error::InvalidWrite {
            reason: "Offscreen.BlockData is neither NULL, TEXT, nor BLOB".to_owned(),
        }),
    }
}

#[cfg(all(feature = "write", feature = "raster"))]
fn validate_raster_canvas_tree(
    connection: &rusqlite::Connection,
    parent_layer_id: i64,
    limits: Limits,
) -> Result<u64> {
    let (canvas_id, parent_folder): (i64, i64) = query_unique_raster_row(
        connection,
        "Layer",
        parent_layer_id,
        "SELECT CanvasId, LayerFolder FROM Layer WHERE MainId = ?1",
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if parent_folder == 0 {
        return Err(Error::InvalidWrite {
            reason: "raster clone parent is not a folder".to_owned(),
        });
    }
    let root_id: i64 = query_unique_raster_row(
        connection,
        "Canvas",
        canvas_id,
        "SELECT CanvasRootFolder FROM Canvas WHERE MainId = ?1",
        |row| row.get(0),
    )?;
    let mut statement = connection.prepare(
        "SELECT MainId, coalesce(LayerFirstChildIndex, 0), \
         coalesce(LayerNextIndex, 0) FROM Layer WHERE CanvasId = ?1",
    )?;
    let rows = statement
        .query_map([canvas_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    enforce_raster_clone_limit(
        rows.len() as u64,
        limits.max_layers(),
        "raster canvas layers",
    )?;
    let mut links = BTreeMap::new();
    for (id, child, next) in rows {
        if id <= 0 || links.insert(id, (child, next)).is_some() {
            return Err(Error::InvalidWrite {
                reason: "raster canvas tree contains an invalid or duplicate layer ID".to_owned(),
            });
        }
    }
    if !links.contains_key(&root_id) {
        return Err(Error::InvalidWrite {
            reason: "CanvasRootFolder is absent from the raster canvas".to_owned(),
        });
    }
    let mut visited = BTreeSet::new();
    let mut stack = vec![(root_id, 0_u64)];
    let mut parent_depth = None;
    while let Some((id, depth)) = stack.pop() {
        if depth > limits.max_layer_tree_depth() {
            return Err(Error::LimitExceeded {
                resource: "raster layer tree depth",
                value: depth,
                limit: limits.max_layer_tree_depth(),
            });
        }
        if !visited.insert(id) {
            return Err(Error::InvalidWrite {
                reason: format!("raster canvas tree reaches layer {id} more than once"),
            });
        }
        if id == parent_layer_id {
            parent_depth = Some(depth);
        }
        let (child, next) = links.get(&id).copied().ok_or_else(|| Error::InvalidWrite {
            reason: format!("raster canvas tree refers to missing layer {id}"),
        })?;
        if next != 0 {
            stack.push((next, depth));
        }
        if child != 0 {
            stack.push((child, depth.checked_add(1).ok_or(Error::OffsetOverflow)?));
        }
    }
    if visited.len() != links.len() || !visited.contains(&parent_layer_id) {
        return Err(Error::InvalidWrite {
            reason: "raster canvas tree contains unreachable layers or parent".to_owned(),
        });
    }
    parent_depth.ok_or_else(|| Error::InvalidWrite {
        reason: "raster clone parent has no resolved tree depth".to_owned(),
    })
}

#[cfg(all(feature = "write", feature = "raster"))]
fn query_unique_raster_row<T, F>(
    connection: &rusqlite::Connection,
    table: &str,
    id: i64,
    sql: &str,
    mapper: F,
) -> Result<T>
where
    F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let count_sql = format!(
        "SELECT count(*) FROM {} WHERE MainId = ?1",
        quote_raster_identifier(table)
    );
    let count: i64 = connection.query_row(&count_sql, [id], |row| row.get(0))?;
    if count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("{table}.MainId {id} resolves to {count} rows"),
        });
    }
    connection.query_row(sql, [id], mapper).map_err(Into::into)
}

#[cfg(all(feature = "write", feature = "raster"))]
fn quote_raster_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(all(feature = "write", feature = "raster"))]
fn enforce_raster_clone_limit(value: u64, limit: u64, resource: &'static str) -> Result<()> {
    if value > limit {
        Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        })
    } else {
        Ok(())
    }
}

#[cfg(feature = "write")]
pub(crate) fn replace_attribute_block_sizes(bytes: &[u8], block_sizes: &[u32]) -> Result<Vec<u8>> {
    let attributes = OffscreenAttributes::parse(bytes)?;
    if attributes.block_sizes().len() != block_sizes.len() {
        return Err(Error::InvalidWrite {
            reason: format!(
                "replacement BlockSize count {} does not match attribute count {}",
                block_sizes.len(),
                attributes.block_sizes().len()
            ),
        });
    }
    let values_size = block_sizes
        .len()
        .checked_mul(4)
        .ok_or(Error::OffsetOverflow)?;
    let values_start = bytes
        .len()
        .checked_sub(values_size)
        .ok_or(Error::OffsetOverflow)?;
    let mut output = bytes.to_vec();
    for (index, value) in block_sizes.iter().enumerate() {
        let offset = values_start
            .checked_add(index.checked_mul(4).ok_or(Error::OffsetOverflow)?)
            .ok_or(Error::OffsetOverflow)?;
        output[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    OffscreenAttributes::parse(&output)?;
    Ok(output)
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
        (1, 1) => Ok(PixelFormat::GrayAlpha8),
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
                PixelFormat::GrayAlpha8 => {
                    let alpha = byte_at(&tile.bytes, source_pixel)?;
                    let value = byte_at(
                        &tile.bytes,
                        tile_area
                            .checked_add(source_pixel)
                            .ok_or(Error::OffsetOverflow)?,
                    )?;
                    let target =
                        usize::try_from(target_pixel.checked_mul(2).ok_or(Error::OffsetOverflow)?)
                            .map_err(|_| Error::OffsetOverflow)?;
                    image.pixels[target] = value;
                    image.pixels[target + 1] = alpha;
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
        attributes_with_packing(1, 4, 5, 32, 8)
    }

    fn attributes_with_packing(
        alpha_channels: u32,
        buffer_channels: u32,
        total_channels: u32,
        buffer_bit_depth: u32,
        alpha_bit_depth: u32,
    ) -> Vec<u8> {
        let mut parameter = Vec::new();
        push_label(&mut parameter, "Parameter");
        for value in [300, 200, 2, 1] {
            push_u32(&mut parameter, value);
        }
        let mut packing = [0_u32; 16];
        packing[1] = alpha_channels;
        packing[2] = buffer_channels;
        packing[3] = total_channels;
        packing[6] = buffer_bit_depth << 5;
        packing[8] = alpha_bit_depth << 5;
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

    #[cfg(all(feature = "write", feature = "raster"))]
    fn raster_clone_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Canvas (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasRootFolder INTEGER
                 );
                 INSERT INTO Canvas VALUES (1, 1, 2);
                 CREATE TABLE Layer (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerName TEXT,
                    LayerType INTEGER,
                    LayerFolder INTEGER,
                    LayerSelect INTEGER,
                    LayerNextIndex INTEGER,
                    LayerFirstChildIndex INTEGER,
                    LayerUuid TEXT,
                    LayerRenderMipmap INTEGER,
                    LayerLayerMaskMipmap INTEGER,
                    LayerRenderThumbnail INTEGER,
                    LayerLayerMaskThumbnail INTEGER,
                    OpaqueColumn BLOB
                 );
                 INSERT INTO Layer VALUES (
                    1, 2, 1, 'root', 256, 1, 0, 0, 3,
                    '00aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaa',
                    0, 0, 0, 0, X'01'
                 );
                 INSERT INTO Layer VALUES (
                    2, 3, 1, 'template', 1, 0, 1, 0, 0,
                    '11bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbb',
                    3, 0, 3, 0, X'CAFE'
                 );
                 CREATE TABLE Mipmap (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    MipmapCount INTEGER,
                    BaseMipmapInfo INTEGER,
                    OpaqueColumn TEXT
                 );
                 INSERT INTO Mipmap VALUES (1, 3, 1, 3, 2, 5, 'mipmap');
                 CREATE TABLE MipmapInfo (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    ThisScale REAL,
                    Offscreen INTEGER,
                    NextIndex INTEGER,
                    OpaqueColumn TEXT
                 );
                 INSERT INTO MipmapInfo VALUES (1, 5, 1, 3, 100.0, 7, 6, 'base');
                 INSERT INTO MipmapInfo VALUES (2, 6, 1, 3, 50.0, 9, 0, 'derived');
                 CREATE TABLE Offscreen (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    Attribute BLOB,
                    BlockData BLOB,
                    OpaqueColumn INTEGER
                 );
                 CREATE TABLE LayerThumbnail (
                    _PW_ID INTEGER PRIMARY KEY,
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    ThumbnailOffscreen INTEGER,
                    OpaqueColumn TEXT
                 );
                 INSERT INTO LayerThumbnail VALUES (1, 3, 1, 3, 8, 'thumbnail');
                 CREATE TABLE ExternalChunk (
                    _PW_ID INTEGER PRIMARY KEY,
                    ExternalID BLOB,
                    Offset INTEGER
                 );
                 CREATE TABLE ElemScheme (
                    _PW_ID INTEGER PRIMARY KEY,
                    TableName TEXT,
                    MaxIndex INTEGER
                 );
                 INSERT INTO ElemScheme VALUES (1, 'Layer', 3);
                 INSERT INTO ElemScheme VALUES (2, 'Mipmap', 3);
                 INSERT INTO ElemScheme VALUES (3, 'MipmapInfo', 6);
                 INSERT INTO ElemScheme VALUES (4, 'Offscreen', 9);
                 INSERT INTO ElemScheme VALUES (5, 'LayerThumbnail', 3);",
            )
            .unwrap();
        let attribute = attributes();
        for (pw_id, main_id, identifier, opaque) in [
            (
                1_i64,
                7_i64,
                b"extrnlidAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".as_slice(),
                100_i64,
            ),
            (
                2,
                9,
                b"extrnlidBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".as_slice(),
                50,
            ),
            (
                3,
                8,
                b"extrnlidCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC".as_slice(),
                25,
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO Offscreen VALUES (?1, ?2, 1, 3, ?3, ?4, ?5)",
                    params![pw_id, main_id, &attribute, identifier, opaque],
                )
                .unwrap();
        }
        connection
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    fn raster_clone_insertion<'a>(
        connection: &Connection,
        template: &'a RasterLayerTemplate,
    ) -> RasterLayerCloneInsertion<'a> {
        RasterLayerCloneInsertion::prepare(
            connection,
            template,
            "new raster",
            attributes(),
            vec![
                b"extrnlid11111111111111111111111111111111".to_vec(),
                b"extrnlid22222222222222222222222222222222".to_vec(),
                b"extrnlid33333333333333333333333333333333".to_vec(),
            ],
            Limits::default(),
        )
        .unwrap()
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
    fn recognizes_observed_eight_bit_gray_alpha_packing() {
        let attributes =
            OffscreenAttributes::parse(&attributes_with_packing(1, 1, 2, 8, 8)).unwrap();
        assert_eq!(
            pixel_format(attributes.packing()).unwrap(),
            PixelFormat::GrayAlpha8
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
        assert!(RasterDataState::Constructed.is_constructed());
        assert!(RasterDataState::MissingReference.is_default_filled());
        assert!(RasterDataState::MissingExternalChunk.is_default_filled());
        assert!(!RasterDataState::Constructed.is_default_filled());
        assert!(!RasterDataState::Present.is_default_filled());
        assert!(!RasterDataState::MissingReference.is_present());
        assert!(!RasterDataState::MissingExternalChunk.is_present());
        assert!(RasterDataState::Present.is_present());
    }

    #[test]
    fn constructs_raster_images_with_validated_dimensions() {
        let image = RasterImage::from_pixels(2, 1, PixelFormat::GrayAlpha8, [1, 2, 3, 4]).unwrap();
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 1);
        assert_eq!(image.byte_len(), 4);
        assert!(image.data_state().is_constructed());
        assert!(matches!(
            RasterImage::from_pixels(2, 1, PixelFormat::Rgba8, [0; 4]),
            Err(Error::InvalidRaster { .. })
        ));
        assert!(matches!(
            RasterImage::from_pixels(0, 1, PixelFormat::Gray8, []),
            Err(Error::InvalidRaster { .. })
        ));
    }

    #[test]
    fn reads_and_edits_rgba8_pixels_by_named_channels() {
        let mut image = RasterImage {
            width: 2,
            height: 1,
            format: PixelFormat::Rgba8,
            state: RasterDataState::Present,
            pixels: vec![0, 10, 20, 30, 250, 240, 230, 220],
        };

        assert_eq!(
            image.rgba8_pixels().unwrap().collect::<Vec<_>>(),
            vec![
                Rgba8Pixel {
                    r: 0,
                    g: 10,
                    b: 20,
                    a: 30,
                },
                Rgba8Pixel {
                    r: 250,
                    g: 240,
                    b: 230,
                    a: 220,
                },
            ]
        );
        assert!(image.gray8_pixels().is_none());
        assert!(image.gray8_pixels_mut().is_none());

        for mut pixel in image.rgba8_pixels_mut().unwrap() {
            pixel.invert();
        }
        assert_eq!(image.pixels(), &[255, 245, 235, 30, 5, 15, 25, 220]);
        assert_eq!(Rgba8Pixel::MIN.r, Rgba8Pixel::CHANNEL_MIN);
        assert_eq!(Rgba8Pixel::MAX.a, Rgba8Pixel::CHANNEL_MAX);
        let pixel = Rgba8Pixel {
            r: 250,
            g: 20,
            b: 30,
            a: 40,
        };
        assert!(pixel.checked_add_rgb(10).is_none());
        assert!(pixel.checked_sub_rgb(21).is_none());
        assert_eq!(
            pixel.saturating_add_rgb(10),
            Rgba8Pixel {
                r: 255,
                g: 30,
                b: 40,
                a: 40,
            }
        );
        assert_eq!(
            pixel.saturating_sub_rgb(25),
            Rgba8Pixel {
                r: 225,
                g: 0,
                b: 5,
                a: 40,
            }
        );
    }

    #[test]
    fn reads_and_edits_gray8_pixels_by_named_values() {
        let mut image = RasterImage {
            width: 3,
            height: 1,
            format: PixelFormat::Gray8,
            state: RasterDataState::Present,
            pixels: vec![0, 128, 255],
        };

        assert_eq!(
            image.gray8_pixels().unwrap().collect::<Vec<_>>(),
            vec![
                Gray8Pixel { value: 0 },
                Gray8Pixel { value: 128 },
                Gray8Pixel { value: 255 },
            ]
        );
        assert!(image.rgba8_pixels().is_none());
        assert!(image.rgba8_pixels_mut().is_none());

        for mut pixel in image.gray8_pixels_mut().unwrap() {
            pixel.invert();
        }
        assert_eq!(image.pixels(), &[255, 127, 0]);
        assert_eq!(Gray8Pixel::MIN.value, Gray8Pixel::CHANNEL_MIN);
        assert_eq!(Gray8Pixel::MAX.value, Gray8Pixel::CHANNEL_MAX);
        let pixel = Gray8Pixel { value: 250 };
        assert!(pixel.checked_add(6).is_none());
        assert_eq!(pixel.checked_sub(50), Some(Gray8Pixel { value: 200 }));
        assert_eq!(pixel.saturating_add(10), Gray8Pixel::MAX);
        assert_eq!(Gray8Pixel { value: 5 }.saturating_sub(10), Gray8Pixel::MIN);
    }

    #[test]
    fn reads_and_edits_gray_alpha8_pixels_by_named_channels() {
        let mut image = RasterImage {
            width: 2,
            height: 1,
            format: PixelFormat::GrayAlpha8,
            state: RasterDataState::Present,
            pixels: vec![10, 20, 240, 230],
        };

        assert_eq!(
            image.gray_alpha8_pixels().unwrap().collect::<Vec<_>>(),
            vec![
                GrayAlpha8Pixel {
                    value: 10,
                    alpha: 20,
                },
                GrayAlpha8Pixel {
                    value: 240,
                    alpha: 230,
                },
            ]
        );
        assert!(image.rgba8_pixels().is_none());
        assert!(image.gray8_pixels().is_none());

        for mut pixel in image.gray_alpha8_pixels_mut().unwrap() {
            pixel.invert();
        }
        assert_eq!(image.pixels(), &[245, 20, 15, 230]);

        let pixel = GrayAlpha8Pixel {
            value: 250,
            alpha: 42,
        };
        assert!(pixel.checked_add_value(10).is_none());
        assert!(pixel.checked_sub_value(251).is_none());
        assert_eq!(
            pixel.saturating_add_value(10),
            GrayAlpha8Pixel {
                value: 255,
                alpha: 42,
            }
        );
        assert_eq!(
            pixel.saturating_sub_value(251),
            GrayAlpha8Pixel {
                value: 0,
                alpha: 42,
            }
        );
    }

    #[test]
    fn common_pixel_iterators_cover_all_supported_formats() {
        let mut rgba = RasterImage {
            width: 1,
            height: 1,
            format: PixelFormat::Rgba8,
            state: RasterDataState::Present,
            pixels: vec![250, 20, 30, 40],
        };
        assert_eq!(
            rgba.pixel_iter().collect::<Vec<_>>(),
            vec![RasterPixel::Rgba8(Rgba8Pixel {
                r: 250,
                g: 20,
                b: 30,
                a: 40,
            })]
        );
        {
            let mut pixels = rgba.pixel_iter_mut();
            assert_eq!(pixels.len(), 1);
            let mut pixel = pixels.next().unwrap();
            assert_eq!(pixel.format(), PixelFormat::Rgba8);
            assert!(!pixel.checked_add_assign(10));
            pixel.saturating_add_assign(10);
        }
        assert_eq!(rgba.pixels(), &[255, 30, 40, 40]);

        let mut gray = RasterImage {
            width: 1,
            height: 1,
            format: PixelFormat::Gray8,
            state: RasterDataState::Present,
            pixels: vec![5],
        };
        {
            let mut pixel = gray.pixel_iter_mut().next().unwrap();
            assert_eq!(pixel.format(), PixelFormat::Gray8);
            assert!(!pixel.checked_sub_assign(10));
            pixel.saturating_sub_assign(10);
        }
        assert_eq!(gray.pixels(), &[0]);

        let mut gray_alpha = RasterImage {
            width: 1,
            height: 1,
            format: PixelFormat::GrayAlpha8,
            state: RasterDataState::Present,
            pixels: vec![5, 42],
        };
        {
            let mut pixel = gray_alpha.pixel_iter_mut().next().unwrap();
            assert_eq!(pixel.format(), PixelFormat::GrayAlpha8);
            pixel.invert();
        }
        assert_eq!(gray_alpha.pixels(), &[250, 42]);
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn encodes_rgba_pixels_into_native_tiles_and_preserves_padding() {
        let attributes = OffscreenAttributes::parse(&attributes()).unwrap();
        let mut pixels = vec![0_u8; 300 * 200 * 4];
        pixels[0..4].copy_from_slice(&[1, 2, 3, 4]);
        let second_tile_pixel = (256 * 4) as usize;
        pixels[second_tile_pixel..second_tile_pixel + 4].copy_from_slice(&[5, 6, 7, 8]);
        let encoder = RasterEncoder::new(
            &attributes,
            PixelFormat::Rgba8,
            &pixels,
            4096,
            1024 * 1024,
            1024 * 1024,
        )
        .unwrap();
        assert_eq!(encoder.tile_count(), 2);

        let tile_area = 256 * 256;
        let first = encoder
            .encode_tile(0, Some(vec![0xAA; tile_area * 5]))
            .unwrap();
        assert_eq!(first[0], 4);
        assert_eq!(&first[tile_area..tile_area + 4], &[3, 2, 1, 0xAA]);
        assert_eq!(first[tile_area - 1], 0xAA);

        let second = encoder.encode_tile(1, None).unwrap();
        assert_eq!(second[0], 8);
        assert_eq!(&second[tile_area..tile_area + 4], &[7, 6, 5, 0]);
        assert_eq!(second[255], 0);
        assert_eq!(second[tile_area + 255 * 4], 0);
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn encodes_gray_alpha_pixels_into_planar_native_tiles() {
        let attributes =
            OffscreenAttributes::parse(&attributes_with_packing(1, 1, 2, 8, 8)).unwrap();
        let mut pixels = vec![0_u8; 300 * 200 * 2];
        pixels[0..2].copy_from_slice(&[10, 20]);
        let second_tile_pixel = (256 * 2) as usize;
        pixels[second_tile_pixel..second_tile_pixel + 2].copy_from_slice(&[30, 40]);
        let encoder = RasterEncoder::new(
            &attributes,
            PixelFormat::GrayAlpha8,
            &pixels,
            4096,
            1024 * 1024,
            1024 * 1024,
        )
        .unwrap();

        let tile_area = 256 * 256;
        let first = encoder
            .encode_tile(0, Some(vec![0xAA; tile_area * 2]))
            .unwrap();
        assert_eq!(first[0], 20);
        assert_eq!(first[tile_area], 10);
        assert_eq!(first[tile_area - 1], 0xAA);

        let second = encoder.encode_tile(1, None).unwrap();
        assert_eq!(second[0], 40);
        assert_eq!(second[tile_area], 30);
        assert_eq!(second[255], 0);
        assert_eq!(second[tile_area + 255], 0);
    }

    #[test]
    fn decodes_planar_gray_alpha_tiles_into_interleaved_pixels() {
        let attributes =
            OffscreenAttributes::parse(&attributes_with_packing(1, 1, 2, 8, 8)).unwrap();
        let tile_area = 256 * 256;
        let mut bytes = vec![0_u8; tile_area * 2];
        bytes[0] = 20;
        bytes[tile_area] = 10;
        let tile = DecodedTile {
            index: 0,
            parameters: BlockParameters::from_raw([0, 2, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0]),
            bytes,
        };
        let mut image = RasterImage {
            width: 300,
            height: 200,
            format: PixelFormat::GrayAlpha8,
            state: RasterDataState::Present,
            pixels: vec![0; 300 * 200 * 2],
        };

        copy_tile(&mut image, &attributes, &tile).unwrap();
        assert_eq!(&image.pixels()[..2], &[10, 20]);
    }

    #[cfg(feature = "image")]
    #[test]
    fn converts_supported_rasters_to_image_rs_without_changing_bytes() {
        for (format, pixels) in [
            (PixelFormat::Rgba8, vec![1, 2, 3, 4]),
            (PixelFormat::Gray8, vec![5]),
            (PixelFormat::GrayAlpha8, vec![6, 7]),
        ] {
            let dynamic = RasterImage {
                width: 1,
                height: 1,
                format,
                state: RasterDataState::Present,
                pixels: pixels.clone(),
            }
            .into_dynamic_image();
            assert_eq!(dynamic.width(), 1);
            assert_eq!(dynamic.height(), 1);
            assert_eq!(dynamic.as_bytes(), pixels);
        }

        let rgb = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(1, 1, vec![10, 20, 30]).unwrap(),
        );
        let raster = RasterImage::try_from_dynamic_image(rgb).unwrap();
        assert_eq!(raster.format(), PixelFormat::Rgba8);
        assert_eq!(raster.pixels(), &[10, 20, 30, 255]);
        assert!(raster.data_state().is_constructed());

        let unsupported = image::DynamicImage::ImageRgba16(
            image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(1, 1, vec![0; 4]).unwrap(),
        );
        assert!(matches!(
            RasterImage::try_from_dynamic_image(unsupported),
            Err(Error::UnsupportedRaster { .. })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn replaces_only_attribute_block_size_values() {
        let original = attributes();
        let updated = replace_attribute_block_sizes(&original, &[1234, 5678]).unwrap();
        assert_eq!(
            OffscreenAttributes::parse(&updated).unwrap().block_sizes(),
            &[1234, 5678]
        );
        let value_bytes = 2 * 4;
        assert_eq!(
            &updated[..updated.len() - value_bytes],
            &original[..original.len() - value_bytes]
        );
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn rejects_a_raster_replacement_with_the_wrong_shape() {
        let attributes = OffscreenAttributes::parse(&attributes()).unwrap();
        assert!(matches!(
            RasterEncoder::new(
                &attributes,
                PixelFormat::Gray8,
                &[],
                4096,
                1024 * 1024,
                1024 * 1024,
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            RasterEncoder::new(
                &attributes,
                PixelFormat::Rgba8,
                &[0; 4],
                4096,
                1024 * 1024,
                1024 * 1024,
            ),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn enforces_the_layer_limit_before_cloning_raster_rows() {
        let connection = raster_clone_database();
        assert!(matches!(
            RasterLayerTemplate::read(&connection, 3, 2, Limits::default().with_max_layers(2),),
            Err(Error::LimitExceeded {
                resource: "layers after raster clone",
                value: 3,
                limit: 2,
            })
        ));
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn enforces_the_new_raster_layer_depth_at_the_parent_boundary() {
        let connection = raster_clone_database();
        connection
            .execute_batch(
                "UPDATE Layer SET LayerNextIndex = 4 WHERE MainId = 3;
                 INSERT INTO Layer VALUES (
                    3, 4, 1, 'destination', 256, 1, 0, 0, 0,
                    '22cccccccc-cccc-4ccc-8ccc-cccccccccc',
                    0, 0, 0, 0, X'02'
                 );
                 UPDATE ElemScheme SET MaxIndex = 4 WHERE TableName = 'Layer';",
            )
            .unwrap();

        assert!(
            RasterLayerTemplate::read(
                &connection,
                3,
                4,
                Limits::default().with_max_layer_tree_depth(2),
            )
            .is_ok()
        );
        assert!(matches!(
            RasterLayerTemplate::read(
                &connection,
                3,
                4,
                Limits::default().with_max_layer_tree_depth(1),
            ),
            Err(Error::LimitExceeded {
                resource: "new raster layer tree depth",
                value: 2,
                limit: 1,
            })
        ));
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn clones_raster_rows_preserves_unknown_columns_and_invalidates_caches() {
        let mut connection = raster_clone_database();
        let template = RasterLayerTemplate::read(&connection, 3, 2, Limits::default()).unwrap();
        let insertion = raster_clone_insertion(&connection, &template);
        let layer_id = insert_raster_layer_clone(&mut connection, insertion).unwrap();
        assert_eq!(layer_id, 4);
        let layer: (i64, i64, i64, String, Vec<u8>) = connection
            .query_row(
                "SELECT LayerNextIndex, LayerRenderMipmap, LayerRenderThumbnail, \
                 LayerUuid, OpaqueColumn FROM Layer WHERE MainId = 4",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((layer.0, layer.1, layer.2), (3, 4, 4));
        assert_eq!(layer.3.len(), 36);
        assert_eq!(layer.4, [0xCA, 0xFE]);
        assert_eq!(
            connection
                .query_row(
                    "SELECT LayerFirstChildIndex FROM Layer WHERE MainId = 2",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            4
        );
        let offscreens = connection
            .prepare(
                "SELECT MainId, BlockData, OpaqueColumn FROM Offscreen \
                 WHERE LayerId = 4 ORDER BY MainId",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            offscreens,
            vec![
                (
                    10,
                    b"extrnlid11111111111111111111111111111111".to_vec(),
                    100,
                ),
                (11, b"extrnlid22222222222222222222222222222222".to_vec(), 50,),
                (12, b"extrnlid33333333333333333333333333333333".to_vec(), 25,),
            ]
        );
        let maxima = connection
            .prepare(
                "SELECT TableName, MaxIndex FROM ElemScheme \
                 WHERE TableName IN ('Layer','Mipmap','MipmapInfo','Offscreen','LayerThumbnail') \
                 ORDER BY TableName",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            maxima,
            vec![
                ("Layer".to_owned(), 4),
                ("LayerThumbnail".to_owned(), 4),
                ("Mipmap".to_owned(), 4),
                ("MipmapInfo".to_owned(), 8),
                ("Offscreen".to_owned(), 12),
            ]
        );
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn preserves_text_storage_for_cloned_raster_external_identifiers() {
        let mut connection = raster_clone_database();
        connection
            .execute(
                "UPDATE Offscreen SET BlockData = CAST(BlockData AS TEXT)",
                [],
            )
            .unwrap();
        let template = RasterLayerTemplate::read(&connection, 3, 2, Limits::default()).unwrap();
        let insertion = raster_clone_insertion(&connection, &template);
        insert_raster_layer_clone(&mut connection, insertion).unwrap();

        let rows = connection
            .prepare(
                "SELECT typeof(BlockData), BlockData FROM Offscreen \
                 WHERE LayerId = 4 ORDER BY MainId",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    "text".to_owned(),
                    "extrnlid11111111111111111111111111111111".to_owned(),
                ),
                (
                    "text".to_owned(),
                    "extrnlid22222222222222222222222222222222".to_owned(),
                ),
                (
                    "text".to_owned(),
                    "extrnlid33333333333333333333333333333333".to_owned(),
                ),
            ]
        );
    }

    #[cfg(all(feature = "write", feature = "raster"))]
    #[test]
    fn rolls_back_every_raster_clone_row_when_a_late_insert_fails() {
        let mut connection = raster_clone_database();
        let template = RasterLayerTemplate::read(&connection, 3, 2, Limits::default()).unwrap();
        let insertion = raster_clone_insertion(&connection, &template);
        connection
            .execute_batch(
                "CREATE TRIGGER reject_second_cloned_offscreen \
                 BEFORE INSERT ON Offscreen WHEN NEW.MainId = 11 \
                 BEGIN SELECT RAISE(ABORT, 'test rollback'); END;",
            )
            .unwrap();
        assert!(insert_raster_layer_clone(&mut connection, insertion).is_err());
        for (table, count) in [
            ("Layer", 2),
            ("Mipmap", 1),
            ("MipmapInfo", 2),
            ("Offscreen", 3),
            ("LayerThumbnail", 1),
        ] {
            let actual: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(actual, count, "{table}");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT LayerFirstChildIndex FROM Layer WHERE MainId = 2",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT MaxIndex FROM ElemScheme WHERE TableName = 'Layer'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3
        );
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
