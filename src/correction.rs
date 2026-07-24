use rusqlite::{params, types::ValueRef};

use crate::{Database, Error, Limits, Result};

const LEVEL_RECORD_SIZE: usize = 10;
const TONE_CURVE_RECORD_SIZE: usize = 130;
const TONE_CURVE_MAX_POINTS: usize = 32;

/// One five-value channel in a level correction.
///
/// CLIP stores these values as unsigned 8.8-style words. The raw words are
/// preserved; the common high-byte representation is available separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorrectionLevel {
    input_left: u16,
    input_mid: u16,
    input_right: u16,
    output_left: u16,
    output_right: u16,
}

impl CorrectionLevel {
    /// Raw input black-point word.
    #[must_use]
    pub const fn input_left_raw(self) -> u16 {
        self.input_left
    }

    /// Raw input midpoint word.
    #[must_use]
    pub const fn input_mid_raw(self) -> u16 {
        self.input_mid
    }

    /// Raw input white-point word.
    #[must_use]
    pub const fn input_right_raw(self) -> u16 {
        self.input_right
    }

    /// Raw output black-point word.
    #[must_use]
    pub const fn output_left_raw(self) -> u16 {
        self.output_left
    }

    /// Raw output white-point word.
    #[must_use]
    pub const fn output_right_raw(self) -> u16 {
        self.output_right
    }

    /// High-byte input black point used by the observed UI.
    #[must_use]
    pub const fn input_left_8bit(self) -> u8 {
        (self.input_left >> 8) as u8
    }

    /// High-byte input midpoint used by the observed UI.
    #[must_use]
    pub const fn input_mid_8bit(self) -> u8 {
        (self.input_mid >> 8) as u8
    }

    /// High-byte input white point used by the observed UI.
    #[must_use]
    pub const fn input_right_8bit(self) -> u8 {
        (self.input_right >> 8) as u8
    }

    /// High-byte output black point used by the observed UI.
    #[must_use]
    pub const fn output_left_8bit(self) -> u8 {
        (self.output_left >> 8) as u8
    }

    /// High-byte output white point used by the observed UI.
    #[must_use]
    pub const fn output_right_8bit(self) -> u8 {
        (self.output_right >> 8) as u8
    }
}

/// One point in a tone-correction curve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorrectionCurvePoint {
    input: u16,
    output: u16,
}

impl CorrectionCurvePoint {
    /// Raw stored input word.
    #[must_use]
    pub const fn input_raw(self) -> u16 {
        self.input
    }

    /// Raw stored output word.
    #[must_use]
    pub const fn output_raw(self) -> u16 {
        self.output
    }

    /// High-byte input coordinate used by the observed UI.
    #[must_use]
    pub const fn input_8bit(self) -> u8 {
        (self.input >> 8) as u8
    }

    /// High-byte output coordinate used by the observed UI.
    #[must_use]
    pub const fn output_8bit(self) -> u8 {
        (self.output >> 8) as u8
    }
}

/// Ordered points for one tone-correction channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrectionCurve {
    points: Vec<CorrectionCurvePoint>,
}

impl CorrectionCurve {
    /// Curve points in stored order.
    #[must_use]
    pub fn points(&self) -> &[CorrectionCurvePoint] {
        &self.points
    }
}

/// Cyan/magenta/yellow offsets for one color-balance range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorBalanceAdjustment {
    cyan: i32,
    magenta: i32,
    yellow: i32,
}

impl ColorBalanceAdjustment {
    /// Cyan/red axis value.
    #[must_use]
    pub const fn cyan(self) -> i32 {
        self.cyan
    }

    /// Magenta/green axis value.
    #[must_use]
    pub const fn magenta(self) -> i32 {
        self.magenta
    }

    /// Yellow/blue axis value.
    #[must_use]
    pub const fn yellow(self) -> i32 {
        self.yellow
    }
}

/// One double-precision curve point attached to a gradient-map stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorrectionGradientPoint {
    input: f64,
    output: f64,
}

impl CorrectionGradientPoint {
    /// Stored input coordinate.
    #[must_use]
    pub const fn input(self) -> f64 {
        self.input
    }

    /// Stored output coordinate.
    #[must_use]
    pub const fn output(self) -> f64 {
        self.output
    }
}

/// One color stop in a gradient-map correction.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionGradientStop {
    red: u32,
    green: u32,
    blue: u32,
    opacity: u32,
    current_color: i32,
    position: i32,
    curve_points: Vec<CorrectionGradientPoint>,
}

impl CorrectionGradientStop {
    /// Raw red component word.
    #[must_use]
    pub const fn red_raw(&self) -> u32 {
        self.red
    }

    /// Raw green component word.
    #[must_use]
    pub const fn green_raw(&self) -> u32 {
        self.green
    }

    /// Raw blue component word.
    #[must_use]
    pub const fn blue_raw(&self) -> u32 {
        self.blue
    }

    /// Raw opacity word.
    #[must_use]
    pub const fn opacity_raw(&self) -> u32 {
        self.opacity
    }

    /// High-byte RGB components used by the observed UI.
    #[must_use]
    pub const fn rgb_8bit(&self) -> [u8; 3] {
        [
            (self.red >> 24) as u8,
            (self.green >> 24) as u8,
            (self.blue >> 24) as u8,
        ]
    }

    /// High-byte opacity used by the observed UI.
    #[must_use]
    pub const fn opacity_8bit(&self) -> u8 {
        (self.opacity >> 24) as u8
    }

    /// Raw current-color flag.
    #[must_use]
    pub const fn current_color(&self) -> i32 {
        self.current_color
    }

    /// Raw fixed-point stop position, where 32768 represents 100%.
    #[must_use]
    pub const fn position_raw(&self) -> i32 {
        self.position
    }

    /// Stop position as a percentage.
    #[must_use]
    pub fn position_percent(&self) -> f64 {
        f64::from(self.position) * 100.0 / 32_768.0
    }

    /// Optional interpolation curve points.
    #[must_use]
    pub fn curve_points(&self) -> &[CorrectionGradientPoint] {
        &self.curve_points
    }
}

/// Parsed `Layer.FilterLayerInfo` correction parameters.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Correction {
    /// Kind 1.
    BrightnessContrast {
        /// Brightness adjustment.
        brightness: i32,
        /// Contrast adjustment.
        contrast: i32,
    },
    /// Kind 2; channels are RGB, red, green, and blue.
    Levels {
        /// The four meaningful channel records.
        channels: [CorrectionLevel; 4],
    },
    /// Kind 3; channels are RGB, red, green, and blue.
    ToneCurve {
        /// The four meaningful channel curves.
        channels: [CorrectionCurve; 4],
    },
    /// Kind 4.
    HueSaturationLuminosity {
        /// Hue adjustment.
        hue: i32,
        /// Saturation adjustment.
        saturation: i32,
        /// Luminosity adjustment.
        luminosity: i32,
    },
    /// Kind 5.
    ColorBalance {
        /// Raw keep-luminosity flag.
        keep_luminosity: i32,
        /// Shadow-range offsets.
        shadows: ColorBalanceAdjustment,
        /// Midtone-range offsets.
        midtones: ColorBalanceAdjustment,
        /// Highlight-range offsets.
        highlights: ColorBalanceAdjustment,
    },
    /// Kind 6.
    ReverseGradient,
    /// Kind 7.
    Posterization {
        /// Requested level count.
        levels: i32,
    },
    /// Kind 8.
    Threshold {
        /// Threshold value.
        level: i32,
    },
    /// Kind 9.
    GradientMap {
        /// Ordered gradient stops.
        stops: Vec<CorrectionGradientStop>,
    },
    /// A future correction kind whose bounded payload is preserved.
    Unknown {
        /// Original big-endian kind value.
        kind: i32,
        /// Bytes after the common kind and section-size header.
        payload: Box<[u8]>,
    },
}

impl Correction {
    /// Original numeric correction kind.
    #[must_use]
    pub const fn kind(&self) -> i32 {
        match self {
            Self::BrightnessContrast { .. } => 1,
            Self::Levels { .. } => 2,
            Self::ToneCurve { .. } => 3,
            Self::HueSaturationLuminosity { .. } => 4,
            Self::ColorBalance { .. } => 5,
            Self::ReverseGradient => 6,
            Self::Posterization { .. } => 7,
            Self::Threshold { .. } => 8,
            Self::GradientMap { .. } => 9,
            Self::Unknown { kind, .. } => *kind,
        }
    }
}

/// One correction layer and its bounded, validated parameter payload.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionLayerData {
    layer_id: i64,
    layer_type: i64,
    special_render_type: Option<i64>,
    format_v132: Option<i64>,
    correction: Correction,
    raw_attributes: Box<[u8]>,
}

impl CorrectionLayerData {
    /// `Layer.MainId`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Original `Layer.LayerType` flags.
    #[must_use]
    pub const fn layer_type(&self) -> i64 {
        self.layer_type
    }

    /// Whether the observed correction-layer bit is set.
    #[must_use]
    pub const fn has_correction_layer_bit(&self) -> bool {
        self.layer_type & 4_096 != 0
    }

    /// Optional raw `Layer.SpecialRenderType`.
    #[must_use]
    pub const fn special_render_type(&self) -> Option<i64> {
        self.special_render_type
    }

    /// Optional raw `Layer.FilterLayerV132`.
    #[must_use]
    pub const fn format_v132(&self) -> Option<i64> {
        self.format_v132
    }

    /// Parsed correction parameters.
    #[must_use]
    pub const fn correction(&self) -> &Correction {
        &self.correction
    }

    /// Original `FilterLayerInfo` bytes.
    #[must_use]
    pub fn raw_attributes(&self) -> &[u8] {
        &self.raw_attributes
    }
}

impl Database {
    /// Reads and validates correction parameters for one layer.
    ///
    /// Files without the optional correction column, unknown layer IDs, and
    /// rows with a `NULL` payload return `None`.
    pub fn correction_layer(
        &self,
        layer_id: i64,
        limits: Limits,
    ) -> Result<Option<CorrectionLayerData>> {
        if !self.schema().has_column("Layer", "FilterLayerInfo") {
            return Ok(None);
        }
        self.require_column("Layer", "MainId")?;
        self.require_column("Layer", "LayerType")?;

        let special = optional_column(self, "SpecialRenderType");
        let v132 = optional_column(self, "FilterLayerV132");
        let metadata_sql = format!(
            "SELECT length(FilterLayerInfo), LayerType, {special}, {v132} \
             FROM Layer WHERE MainId = ?1 LIMIT 1"
        );
        let mut statement = self.connection().prepare(&metadata_sql)?;
        let mut rows = statement.query(params![layer_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let Some(raw_size) = row.get::<_, Option<i64>>(0)? else {
            return Ok(None);
        };
        let size =
            u64::try_from(raw_size).map_err(|_| correction_error("negative payload size"))?;
        if size > limits.max_correction_bytes() {
            return Err(Error::LimitExceeded {
                resource: "correction-layer bytes",
                value: size,
                limit: limits.max_correction_bytes(),
            });
        }
        let layer_type = row.get(1)?;
        let special_render_type = row.get(2)?;
        let format_v132 = row.get(3)?;
        drop(rows);
        drop(statement);

        let mut statement = self
            .connection()
            .prepare("SELECT FilterLayerInfo FROM Layer WHERE MainId = ?1 LIMIT 1")?;
        let raw_attributes =
            statement.query_row(params![layer_id], |row| match row.get_ref(0)? {
                ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(bytes.to_vec()),
                value => Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "FilterLayerInfo".to_owned(),
                    value.data_type(),
                )),
            })?;
        if raw_attributes.len() as u64 != size {
            return Err(correction_error("payload length changed while reading"));
        }
        let correction = parse_correction(&raw_attributes, limits)?;
        Ok(Some(CorrectionLayerData {
            layer_id,
            layer_type,
            special_render_type,
            format_v132,
            correction,
            raw_attributes: raw_attributes.into_boxed_slice(),
        }))
    }
}

fn optional_column(database: &Database, column: &str) -> &'static str {
    match column {
        "SpecialRenderType" if database.schema().has_column("Layer", column) => "SpecialRenderType",
        "FilterLayerV132" if database.schema().has_column("Layer", column) => "FilterLayerV132",
        _ => "NULL",
    }
}

fn parse_correction(bytes: &[u8], limits: Limits) -> Result<Correction> {
    if bytes.len() as u64 > limits.max_correction_bytes() {
        return Err(Error::LimitExceeded {
            resource: "correction-layer bytes",
            value: bytes.len() as u64,
            limit: limits.max_correction_bytes(),
        });
    }
    let mut parser = Parser::new(bytes);
    let kind = parser.i32()?;
    let section_size = parser.nonnegative_size("section size")?;
    if section_size != parser.remaining() {
        return Err(correction_error(format!(
            "declared section size {section_size} does not match {} remaining bytes",
            parser.remaining()
        )));
    }
    let body = parser.take(section_size)?;
    let mut body = Parser::new(body);
    let correction = match kind {
        1 => {
            require_size(&body, 8, "brightness/contrast")?;
            Correction::BrightnessContrast {
                brightness: body.i32()?,
                contrast: body.i32()?,
            }
        }
        2 => parse_levels(&mut body, limits)?,
        3 => parse_tone_curves(&mut body, limits)?,
        4 => {
            require_size(&body, 12, "hue/saturation/luminosity")?;
            Correction::HueSaturationLuminosity {
                hue: body.i32()?,
                saturation: body.i32()?,
                luminosity: body.i32()?,
            }
        }
        5 => {
            require_size(&body, 40, "color balance")?;
            Correction::ColorBalance {
                keep_luminosity: body.i32()?,
                shadows: parse_balance(&mut body)?,
                midtones: parse_balance(&mut body)?,
                highlights: parse_balance(&mut body)?,
            }
        }
        6 => {
            require_size(&body, 0, "reverse gradient")?;
            Correction::ReverseGradient
        }
        7 => {
            require_size(&body, 4, "posterization")?;
            Correction::Posterization {
                levels: body.i32()?,
            }
        }
        8 => {
            require_size(&body, 4, "threshold")?;
            Correction::Threshold { level: body.i32()? }
        }
        9 => parse_gradient_map(&mut body, limits)?,
        kind => Correction::Unknown {
            kind,
            payload: body.take(body.remaining())?.into(),
        },
    };
    body.finish()?;
    Ok(correction)
}

fn parse_levels(parser: &mut Parser<'_>, limits: Limits) -> Result<Correction> {
    if parser.remaining() % LEVEL_RECORD_SIZE != 0 {
        return Err(correction_error(
            "level section is not a whole number of channel records",
        ));
    }
    let count = parser.remaining() / LEVEL_RECORD_SIZE;
    require_count(count, 4, limits, "level channels")?;
    let mut channels = Vec::with_capacity(count);
    for _ in 0..count {
        channels.push(CorrectionLevel {
            input_left: parser.u16()?,
            input_mid: parser.u16()?,
            input_right: parser.u16()?,
            output_left: parser.u16()?,
            output_right: parser.u16()?,
        });
    }
    let channels = channels
        .into_iter()
        .take(4)
        .collect::<Vec<_>>()
        .try_into()
        .expect("the channel count was checked");
    Ok(Correction::Levels { channels })
}

fn parse_tone_curves(parser: &mut Parser<'_>, limits: Limits) -> Result<Correction> {
    if parser.remaining() % TONE_CURVE_RECORD_SIZE != 0 {
        return Err(correction_error(
            "tone-curve section is not a whole number of channel records",
        ));
    }
    let count = parser.remaining() / TONE_CURVE_RECORD_SIZE;
    require_count(count, 4, limits, "tone-curve channels")?;
    let mut channels = Vec::with_capacity(count);
    for _ in 0..count {
        let point_count = parser.i16()?;
        let point_count = usize::try_from(point_count)
            .map_err(|_| correction_error("negative tone-curve point count"))?;
        if point_count > TONE_CURVE_MAX_POINTS {
            return Err(correction_error(format!(
                "tone-curve point count {point_count} exceeds 32"
            )));
        }
        require_count(point_count, 0, limits, "tone-curve points")?;
        let mut points = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            points.push(CorrectionCurvePoint {
                input: parser.u16()?,
                output: parser.u16()?,
            });
        }
        parser.take(128 - point_count * 4)?;
        channels.push(CorrectionCurve { points });
    }
    let channels = channels
        .into_iter()
        .take(4)
        .collect::<Vec<_>>()
        .try_into()
        .expect("the channel count was checked");
    Ok(Correction::ToneCurve { channels })
}

fn parse_balance(parser: &mut Parser<'_>) -> Result<ColorBalanceAdjustment> {
    Ok(ColorBalanceAdjustment {
        cyan: parser.i32()?,
        magenta: parser.i32()?,
        yellow: parser.i32()?,
    })
}

fn parse_gradient_map(parser: &mut Parser<'_>, limits: Limits) -> Result<Correction> {
    let nested_size = parser.nonnegative_size("gradient section size")?;
    if nested_size != parser.remaining() {
        return Err(correction_error(format!(
            "gradient section size {nested_size} does not match {} remaining bytes",
            parser.remaining()
        )));
    }
    require_token(parser.i32()?, 16, "gradient header")?;
    require_token(parser.i32()?, 28, "gradient stop record size")?;
    let stop_count = parser.nonnegative_size("gradient stop count")?;
    require_count(stop_count, 0, limits, "gradient stops")?;
    require_token(parser.i32()?, 16, "gradient curve record size")?;

    let stop_bytes = stop_count
        .checked_mul(28)
        .ok_or_else(|| correction_error("gradient stop byte count overflow"))?;
    if stop_bytes > parser.remaining() {
        return Err(correction_error("gradient stop records are truncated"));
    }

    struct RawStop {
        red: u32,
        green: u32,
        blue: u32,
        opacity: u32,
        current_color: i32,
        position: i32,
        point_count: usize,
    }

    let mut raw_stops = Vec::with_capacity(stop_count);
    let mut total_points = 0_usize;
    for _ in 0..stop_count {
        let red = parser.u32()?;
        let green = parser.u32()?;
        let blue = parser.u32()?;
        let opacity = parser.u32()?;
        let current_color = parser.i32()?;
        let position = parser.i32()?;
        let point_count = parser.nonnegative_size("gradient curve point count")?;
        total_points = total_points
            .checked_add(point_count)
            .ok_or_else(|| correction_error("gradient point count overflow"))?;
        require_count(total_points, 0, limits, "gradient curve points")?;
        raw_stops.push(RawStop {
            red,
            green,
            blue,
            opacity,
            current_color,
            position,
            point_count,
        });
    }

    let required_point_bytes = total_points
        .checked_mul(16)
        .ok_or_else(|| correction_error("gradient point byte count overflow"))?;
    if parser.remaining() != required_point_bytes {
        return Err(correction_error(format!(
            "gradient point data has {} bytes instead of {required_point_bytes}",
            parser.remaining()
        )));
    }

    let mut stops = Vec::with_capacity(stop_count);
    for raw in raw_stops {
        let mut curve_points = Vec::with_capacity(raw.point_count);
        for _ in 0..raw.point_count {
            let input = parser.f64()?;
            let output = parser.f64()?;
            if !input.is_finite() || !output.is_finite() {
                return Err(correction_error("gradient curve point is not finite"));
            }
            curve_points.push(CorrectionGradientPoint { input, output });
        }
        stops.push(CorrectionGradientStop {
            red: raw.red,
            green: raw.green,
            blue: raw.blue,
            opacity: raw.opacity,
            current_color: raw.current_color,
            position: raw.position,
            curve_points,
        });
    }
    Ok(Correction::GradientMap { stops })
}

fn require_size(parser: &Parser<'_>, expected: usize, name: &str) -> Result<()> {
    if parser.remaining() == expected {
        Ok(())
    } else {
        Err(correction_error(format!(
            "{name} has {} bytes instead of {expected}",
            parser.remaining()
        )))
    }
}

fn require_count(
    count: usize,
    minimum: usize,
    limits: Limits,
    resource: &'static str,
) -> Result<()> {
    if count < minimum {
        return Err(correction_error(format!(
            "{resource} count {count} is below {minimum}"
        )));
    }
    if count as u64 > limits.max_correction_items() {
        return Err(Error::LimitExceeded {
            resource,
            value: count as u64,
            limit: limits.max_correction_items(),
        });
    }
    Ok(())
}

fn require_token(actual: i32, expected: i32, name: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(correction_error(format!(
            "{name} value {actual} does not match {expected}"
        )))
    }
}

fn correction_error(reason: impl Into<String>) -> Error {
    Error::InvalidCorrection {
        reason: reason.into(),
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(size)
            .ok_or_else(|| correction_error("offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| correction_error("payload is truncated"))?;
        self.offset = end;
        Ok(bytes)
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes were taken"),
        ))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes were taken"),
        ))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes were taken"),
        ))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes were taken"),
        ))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes were taken"),
        ))
    }

    fn nonnegative_size(&mut self, name: &str) -> Result<usize> {
        let value = self.i32()?;
        usize::try_from(value).map_err(|_| correction_error(format!("{name} is negative")))
    }

    fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(correction_error(format!(
                "{} trailing correction bytes",
                self.remaining()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn wrap(kind: i32, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(body.len() + 8);
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&(body.len() as i32).to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    fn database(payload: &[u8]) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER,
                    LayerType INTEGER,
                    SpecialRenderType INTEGER,
                    FilterLayerV132 INTEGER,
                    FilterLayerInfo BLOB
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Layer VALUES (1, 4098, 13, NULL, ?1)",
                params![payload],
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn parses_fixed_and_unknown_corrections() {
        let mut body = Vec::new();
        body.extend_from_slice(&(-12_i32).to_be_bytes());
        body.extend_from_slice(&34_i32.to_be_bytes());
        let parsed = parse_correction(&wrap(1, &body), Limits::default()).unwrap();
        assert_eq!(
            parsed,
            Correction::BrightnessContrast {
                brightness: -12,
                contrast: 34
            }
        );

        let parsed = parse_correction(&wrap(77, &[1, 2, 3]), Limits::default()).unwrap();
        assert!(matches!(
            parsed,
            Correction::Unknown {
                kind: 77,
                payload
            } if payload.as_ref() == [1, 2, 3]
        ));
    }

    #[test]
    fn parses_levels_without_discarding_low_bytes() {
        let mut body = Vec::new();
        for channel in 0..4_u16 {
            for value in 0..5_u16 {
                body.extend_from_slice(&(channel * 0x1000 + value * 0x101).to_be_bytes());
            }
        }
        let parsed = parse_correction(&wrap(2, &body), Limits::default()).unwrap();
        let Correction::Levels { channels } = parsed else {
            panic!("expected levels");
        };
        assert_eq!(channels[2].input_mid_raw(), 0x2101);
        assert_eq!(channels[2].input_mid_8bit(), 0x21);
    }

    #[test]
    fn parses_gradient_map_stops_and_points() {
        let mut body = Vec::new();
        body.extend_from_slice(&60_i32.to_be_bytes());
        body.extend_from_slice(&16_i32.to_be_bytes());
        body.extend_from_slice(&28_i32.to_be_bytes());
        body.extend_from_slice(&1_i32.to_be_bytes());
        body.extend_from_slice(&16_i32.to_be_bytes());
        body.extend_from_slice(&0x1200_0000_u32.to_be_bytes());
        body.extend_from_slice(&0x3400_0000_u32.to_be_bytes());
        body.extend_from_slice(&0x5600_0000_u32.to_be_bytes());
        body.extend_from_slice(&0x7800_0000_u32.to_be_bytes());
        body.extend_from_slice(&1_i32.to_be_bytes());
        body.extend_from_slice(&16_384_i32.to_be_bytes());
        body.extend_from_slice(&1_i32.to_be_bytes());
        body.extend_from_slice(&0.25_f64.to_be_bytes());
        body.extend_from_slice(&0.75_f64.to_be_bytes());

        let parsed = parse_correction(&wrap(9, &body), Limits::default()).unwrap();
        let Correction::GradientMap { stops } = parsed else {
            panic!("expected gradient map");
        };
        assert_eq!(stops[0].rgb_8bit(), [0x12, 0x34, 0x56]);
        assert_eq!(stops[0].opacity_8bit(), 0x78);
        assert_eq!(stops[0].position_percent(), 50.0);
        assert_eq!(stops[0].curve_points()[0].input(), 0.25);
    }

    #[test]
    fn reads_layer_metadata_and_enforces_limits() {
        let payload = wrap(8, &127_i32.to_be_bytes());
        let database = database(&payload);
        let layer = database
            .correction_layer(1, Limits::default())
            .unwrap()
            .unwrap();
        assert!(layer.has_correction_layer_bit());
        assert_eq!(layer.special_render_type(), Some(13));
        assert!(matches!(
            layer.correction(),
            Correction::Threshold { level: 127 }
        ));
        assert!(matches!(
            database.correction_layer(1, Limits::default().with_max_correction_bytes(4)),
            Err(Error::LimitExceeded {
                resource: "correction-layer bytes",
                ..
            })
        ));
    }

    #[test]
    fn rejects_malformed_sizes_and_nonfinite_gradient_points() {
        let mut malformed = wrap(1, &[0; 8]);
        malformed[7] = 7;
        assert!(matches!(
            parse_correction(&malformed, Limits::default()),
            Err(Error::InvalidCorrection { .. })
        ));

        let mut body = Vec::new();
        body.extend_from_slice(&60_i32.to_be_bytes());
        body.extend_from_slice(&16_i32.to_be_bytes());
        body.extend_from_slice(&28_i32.to_be_bytes());
        body.extend_from_slice(&1_i32.to_be_bytes());
        body.extend_from_slice(&16_i32.to_be_bytes());
        body.extend_from_slice(&[0; 24]);
        body.extend_from_slice(&1_i32.to_be_bytes());
        body.extend_from_slice(&f64::NAN.to_be_bytes());
        body.extend_from_slice(&0_f64.to_be_bytes());
        assert!(matches!(
            parse_correction(&wrap(9, &body), Limits::default()),
            Err(Error::InvalidCorrection { .. })
        ));
    }
}
