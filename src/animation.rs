#[cfg(feature = "write")]
use std::io::Write;
use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    str,
};

use flate2::read::ZlibDecoder;
#[cfg(feature = "write")]
use flate2::{Compression, write::ZlibEncoder};
use rusqlite::{OptionalExtension, params, types::ValueRef};
#[cfg(feature = "write")]
use rusqlite::{params_from_iter, types::Value};

use crate::{ByteOrder, ClipFile, Database, Error, ExternalBody, Limits, Result};
#[cfg(feature = "write")]
use crate::{ClipWriter, DatabaseSchema, EditableDatabase};

/// One timeline row with validated playback metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    id: i64,
    bank_id: i64,
    next_timeline_id: Option<i64>,
    first_track_id: Option<i64>,
    name: Option<String>,
    frame_rate: f64,
    start_frame: f64,
    end_frame: f64,
    current_frame: Option<f64>,
}

impl Timeline {
    /// `TimeLine.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning animation bank ID.
    #[must_use]
    pub const fn bank_id(&self) -> i64 {
        self.bank_id
    }

    /// Next timeline ID when stored.
    #[must_use]
    pub const fn next_timeline_id(&self) -> Option<i64> {
        self.next_timeline_id
    }

    /// First track ID when stored.
    #[must_use]
    pub const fn first_track_id(&self) -> Option<i64> {
        self.first_track_id
    }

    /// Timeline name when stored.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Frames per second.
    #[must_use]
    pub const fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    /// Inclusive playback-range start in display frames.
    #[must_use]
    pub const fn start_frame(&self) -> f64 {
        self.start_frame
    }

    /// Inclusive playback-range end in display frames.
    #[must_use]
    pub const fn end_frame(&self) -> f64 {
        self.end_frame
    }

    /// Current display frame when stored.
    #[must_use]
    pub const fn current_frame(&self) -> Option<f64> {
        self.current_frame
    }
}

/// One cel-selection keyframe from an `ImageCelName` mixer curve.
#[derive(Clone, Debug, PartialEq)]
pub struct CelKeyframe {
    time_60hz: f32,
    value: f32,
    tag: String,
}

impl CelKeyframe {
    /// Key time in the mixer's observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(&self) -> f32 {
        self.time_60hz
    }

    /// Uninterpreted numeric value paired with the key.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Cel tag stored at this key.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }
}

/// Cel-selection curve associated with one animation-folder layer.
#[derive(Clone, Debug, PartialEq)]
pub struct CelTrack {
    id: i64,
    layer_id: i64,
    keyframes: Vec<CelKeyframe>,
}

/// Raw numeric kind stored in `Track.TrackKind`.
///
/// Only values with independently verified semantics have helper methods. All
/// other values remain available through [`Self::raw`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AnimationTrackKind(i64);

impl AnimationTrackKind {
    /// Creates a track kind without assigning unverified semantics.
    #[must_use]
    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    /// Returns the original `TrackKind` value.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Stable semantic name for a verified track kind.
    ///
    /// Unknown future values return `None`; use [`Self::raw`] when preserving
    /// or diagnosing them.
    #[must_use]
    pub const fn known_name(self) -> Option<&'static str> {
        match self.0 {
            1000 => Some("folder"),
            2000 => Some("image cel"),
            2001 => Some("static image"),
            2003 => Some("paper"),
            2005 => Some("2D camera"),
            4000 => Some("play time"),
            4001 => Some("audio"),
            _ => None,
        }
    }

    /// Whether this is the verified image-cel selection kind (`2000`).
    #[must_use]
    pub const fn is_image_cel(self) -> bool {
        self.0 == 2000
    }

    /// Whether this is the observed non-cel folder kind (`1000`).
    #[must_use]
    pub const fn is_folder(self) -> bool {
        self.0 == 1000
    }

    /// Whether this is the verified static-image-layer kind (`2001`).
    ///
    /// The observed tracks target raster or resizable-image leaf layers and
    /// contain no value curves.
    #[must_use]
    pub const fn is_static_image(self) -> bool {
        self.0 == 2001
    }

    /// Whether this is the verified paper-layer kind (`2003`).
    #[must_use]
    pub const fn is_paper(self) -> bool {
        self.0 == 2003
    }

    /// Whether this is the verified 2D-camera kind (`2005`).
    #[must_use]
    pub const fn is_camera_2d(self) -> bool {
        self.0 == 2005
    }

    /// Whether this is the verified play-time-control kind (`4000`).
    #[must_use]
    pub const fn is_play_time(self) -> bool {
        self.0 == 4000
    }

    /// Whether this is the verified audio-control kind (`4001`).
    #[must_use]
    pub const fn is_audio(self) -> bool {
        self.0 == 4001
    }
}

/// One validated key from a generic animation `FCurve`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationCurveKeyframe {
    time_60hz: f32,
    value: f32,
    tag: Option<String>,
    interpolation: Option<String>,
    left_slope: Option<f32>,
    right_slope: Option<f32>,
    revise_constant: Option<u8>,
}

impl AnimationCurveKeyframe {
    /// Key time in the observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(&self) -> f32 {
        self.time_60hz
    }

    /// Numeric curve value.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Optional string tag associated with the key.
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// Optional interpolation name as stored by CLIP STUDIO PAINT.
    #[must_use]
    pub fn interpolation(&self) -> Option<&str> {
        self.interpolation.as_deref()
    }

    /// Optional incoming slope value.
    #[must_use]
    pub const fn left_slope(&self) -> Option<f32> {
        self.left_slope
    }

    /// Optional outgoing slope value.
    #[must_use]
    pub const fn right_slope(&self) -> Option<f32> {
        self.right_slope
    }

    /// Optional constant-interpolation revision flag.
    #[must_use]
    pub const fn revise_constant(&self) -> Option<u8> {
        self.revise_constant
    }
}

/// Replacement numeric fields for one existing primary animation-curve key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationCurveKeyframeValues {
    time_60hz: f32,
    value: f32,
}

impl AnimationCurveKeyframeValues {
    /// Creates a pair of replacement numeric values.
    #[must_use]
    pub const fn new(time_60hz: f32, value: f32) -> Self {
        Self { time_60hz, value }
    }

    /// Replacement key time in the observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(self) -> f32 {
        self.time_60hz
    }

    /// Replacement numeric curve value.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.value
    }
}

/// Complete field values used when inserting one key into an existing curve.
///
/// Only arrays already present in the selected `FCurve` are written. Optional
/// values must be supplied when their corresponding array exists, which keeps
/// the edit conservative across curve kinds with different field layouts.
#[cfg(feature = "write")]
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationCurveKeyframeInsert {
    time_60hz: f32,
    value: f32,
    tag: Option<String>,
    interpolation: Option<String>,
    left_slope: Option<f32>,
    right_slope: Option<f32>,
    revise_constant: Option<u8>,
}

#[cfg(feature = "write")]
impl AnimationCurveKeyframeInsert {
    /// Creates an insertion containing the required numeric fields.
    #[must_use]
    pub const fn new(time_60hz: f32, value: f32) -> Self {
        Self {
            time_60hz,
            value,
            tag: None,
            interpolation: None,
            left_slope: None,
            right_slope: None,
            revise_constant: None,
        }
    }

    /// Copies every optional field represented by a validated existing key.
    ///
    /// Only the time and numeric value are replaced. This is preferred when
    /// inserting into an existing curve because callers do not need to track
    /// the curve's optional-array layout themselves.
    #[must_use]
    pub fn from_template(template: &AnimationCurveKeyframe, time_60hz: f32, value: f32) -> Self {
        Self {
            time_60hz,
            value,
            tag: template.tag.clone(),
            interpolation: template.interpolation.clone(),
            left_slope: template.left_slope,
            right_slope: template.right_slope,
            revise_constant: template.revise_constant,
        }
    }

    /// Sets the optional string tag field.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Sets the optional interpolation-name field.
    #[must_use]
    pub fn with_interpolation(mut self, interpolation: impl Into<String>) -> Self {
        self.interpolation = Some(interpolation.into());
        self
    }

    /// Sets optional incoming and outgoing slopes.
    #[must_use]
    pub const fn with_slopes(mut self, left: f32, right: f32) -> Self {
        self.left_slope = Some(left);
        self.right_slope = Some(right);
        self
    }

    /// Sets the optional constant-interpolation revision byte.
    #[must_use]
    pub const fn with_revise_constant(mut self, revise_constant: u8) -> Self {
        self.revise_constant = Some(revise_constant);
        self
    }

    /// Key time in the mixer's observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(&self) -> f32 {
        self.time_60hz
    }

    /// Numeric curve value.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }
}

/// One image-cel key used to normalize a template-cloned track.
#[cfg(feature = "write")]
#[derive(Clone, Debug, PartialEq)]
pub struct ImageCelTrackKeyframe {
    time_60hz: f32,
    numeric_value: u32,
    tag: String,
}

#[cfg(feature = "write")]
impl ImageCelTrackKeyframe {
    /// Creates one image-cel key.
    #[must_use]
    pub fn new(time_60hz: f32, numeric_value: u32, tag: impl Into<String>) -> Self {
        Self {
            time_60hz,
            numeric_value,
            tag: tag.into(),
        }
    }

    /// Key time in the mixer's observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(&self) -> f32 {
        self.time_60hz
    }

    /// Integer value paired with the cel tag.
    #[must_use]
    pub const fn numeric_value(&self) -> u32 {
        self.numeric_value
    }

    /// Cel tag stored at this key.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }
}

/// Options for producing an image-cel track from a complete existing template.
#[cfg(feature = "write")]
#[derive(Clone, Debug, PartialEq)]
pub struct ImageCelTrackCloneOptions {
    keyframes: Vec<ImageCelTrackKeyframe>,
}

#[cfg(feature = "write")]
impl ImageCelTrackCloneOptions {
    /// Creates options with the exact non-empty key sequence for the clone.
    #[must_use]
    pub fn new(keyframes: impl IntoIterator<Item = ImageCelTrackKeyframe>) -> Self {
        Self {
            keyframes: keyframes.into_iter().collect(),
        }
    }

    /// Creates a cel sequence while assigning internal numeric values.
    ///
    /// Each input item contains a 60 Hz key time and a target child-layer tag.
    /// Repeated tags reuse the same value; distinct tags receive consecutive
    /// values in first-appearance order. This avoids exposing the redundant
    /// numeric representation required by the on-disk curve and value map.
    pub fn from_timed_cels<I, S>(keyframes: I) -> Result<Self>
    where
        I: IntoIterator<Item = (f32, S)>,
        S: Into<String>,
    {
        let mut values_by_tag = BTreeMap::<String, u32>::new();
        let mut assigned = Vec::new();
        for (time_60hz, tag) in keyframes {
            let tag = tag.into();
            let numeric_value = if let Some(value) = values_by_tag.get(&tag) {
                *value
            } else {
                let value =
                    u32::try_from(values_by_tag.len()).map_err(|_| Error::OffsetOverflow)?;
                if !u32_is_exactly_representable_as_f32(value) {
                    return Err(Error::InvalidWrite {
                        reason: "image-cel sequence has too many distinct tags".to_owned(),
                    });
                }
                values_by_tag.insert(tag.clone(), value);
                value
            };
            assigned.push(ImageCelTrackKeyframe::new(time_60hz, numeric_value, tag));
        }
        if assigned.is_empty() {
            return Err(Error::InvalidWrite {
                reason: "image-cel sequence requires at least one key".to_owned(),
            });
        }
        Ok(Self::new(assigned))
    }

    /// Requested image-cel keys.
    #[must_use]
    pub fn keyframes(&self) -> &[ImageCelTrackKeyframe] {
        &self.keyframes
    }
}

/// One named `FCurve` decoded from a track's primary action mixer.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationCurve {
    kind: String,
    axis: Option<String>,
    keyframes: Vec<AnimationCurveKeyframe>,
}

impl AnimationCurve {
    /// Curve type, such as `ImageCelName`, `PlayTime`, or `AudioPlayer`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Optional component axis for vector parameters, such as `X` or `Y`.
    #[must_use]
    pub fn axis(&self) -> Option<&str> {
        self.axis.as_deref()
    }

    /// Sorted validated keys.
    #[must_use]
    pub fn keyframes(&self) -> &[AnimationCurveKeyframe] {
        &self.keyframes
    }
}

/// One validated double-precision key from a secondary animation `FCurve`.
///
/// Decoded `TrackActionMixer2` value records use the same logical fields as
/// their primary counterparts but store numeric arrays as `Double[]`.
#[derive(Clone, Debug, PartialEq)]
pub struct SecondaryAnimationCurveKeyframe {
    time_60hz: f64,
    value: f64,
    tag: Option<String>,
    interpolation: Option<String>,
    left_slope: Option<f64>,
    right_slope: Option<f64>,
    revise_constant: Option<u8>,
}

impl SecondaryAnimationCurveKeyframe {
    /// Key time in the observed 60 Hz timebase.
    #[must_use]
    pub const fn time_60hz(&self) -> f64 {
        self.time_60hz
    }

    /// Numeric curve value.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Optional string tag associated with the key.
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// Optional interpolation name as stored by CLIP STUDIO PAINT.
    #[must_use]
    pub fn interpolation(&self) -> Option<&str> {
        self.interpolation.as_deref()
    }

    /// Optional incoming slope value.
    #[must_use]
    pub const fn left_slope(&self) -> Option<f64> {
        self.left_slope
    }

    /// Optional outgoing slope value.
    #[must_use]
    pub const fn right_slope(&self) -> Option<f64> {
        self.right_slope
    }

    /// Optional constant-interpolation revision flag.
    #[must_use]
    pub const fn revise_constant(&self) -> Option<u8> {
        self.revise_constant
    }
}

/// One named double-precision `FCurve` decoded from `TrackActionMixer2`.
#[derive(Clone, Debug, PartialEq)]
pub struct SecondaryAnimationCurve {
    kind: String,
    axis: Option<String>,
    keyframes: Vec<SecondaryAnimationCurveKeyframe>,
}

impl SecondaryAnimationCurve {
    /// Curve type, such as the observed `ImageCelName` or `AudioPlayer`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Optional component axis for vector parameters, such as `X` or `Y`.
    #[must_use]
    pub fn axis(&self) -> Option<&str> {
        self.axis.as_deref()
    }

    /// Sorted validated double-precision keys.
    #[must_use]
    pub fn keyframes(&self) -> &[SecondaryAnimationCurveKeyframe] {
        &self.keyframes
    }
}

/// One typed current/default value stored in `TrackValueMap`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AnimationTrackValue {
    /// Type `0`: an IEEE 754 double-precision value.
    Float(f64),
    /// Type `2`: a UTF-16 text value paired with its numeric curve value.
    IndexedText {
        /// Text value, such as an image-cel name.
        text: String,
        /// Numeric value paired with the text in the corresponding curve.
        numeric_value: u32,
    },
    /// Type `3`: two finite IEEE 754 double-precision components.
    Vector2 {
        /// Horizontal component.
        x: f64,
        /// Vertical component.
        y: f64,
    },
    /// A structurally valid value type that this crate does not yet interpret.
    Unknown {
        /// Raw type discriminator.
        kind: u32,
        /// UTF-16 text field stored before the type discriminator.
        text: String,
        /// Remaining big-endian payload bytes.
        payload: Box<[u8]>,
    },
}

/// One finite two-dimensional point or vector used by 2D-camera metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2DPoint {
    x: f64,
    y: f64,
}

impl Camera2DPoint {
    /// Horizontal component.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Vertical component.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// Values evaluated at the saved timeline position in a 2D-camera track's
/// `TrackValueMap`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2DTrackValues {
    image_center: Camera2DPoint,
    image_position: Camera2DPoint,
    rotation: f64,
    scale: f64,
    opacity: f64,
}

impl Camera2DTrackValues {
    /// Camera image center.
    #[must_use]
    pub const fn image_center(self) -> Camera2DPoint {
        self.image_center
    }

    /// Camera image position.
    #[must_use]
    pub const fn image_position(self) -> Camera2DPoint {
        self.image_position
    }

    /// Rotation in degrees.
    #[must_use]
    pub const fn rotation(self) -> f64 {
        self.rotation
    }

    /// Scale as a percentage, where `100.0` is the original size.
    #[must_use]
    pub const fn scale(self) -> f64 {
        self.scale
    }

    /// Opacity as a percentage, where `100.0` is fully opaque.
    #[must_use]
    pub const fn opacity(self) -> f64 {
        self.opacity
    }
}

/// Current 2D-camera transform snapshot from `Camera2DResizableImageInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera2DTransform {
    header_size: u32,
    point_record_size: u32,
    width: u32,
    height: u32,
    scale: Camera2DPoint,
    rotation: f64,
    position: Camera2DPoint,
    image_center: Camera2DPoint,
    corners: Vec<Camera2DPoint>,
    prefix_words: [u32; 5],
    suffix_words: [u32; 6],
    raw: Box<[u8]>,
}

impl Camera2DTransform {
    /// Declared transform-header size.
    #[must_use]
    pub const fn header_size(&self) -> u32 {
        self.header_size
    }

    /// Declared size of each transformed-corner record.
    #[must_use]
    pub const fn point_record_size(&self) -> u32 {
        self.point_record_size
    }

    /// Width stored in the transform snapshot.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height stored in the transform snapshot.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Horizontal and vertical scale factors, where `1.0` is the original size.
    #[must_use]
    pub const fn scale(&self) -> Camera2DPoint {
        self.scale
    }

    /// Rotation in degrees.
    #[must_use]
    pub const fn rotation(&self) -> f64 {
        self.rotation
    }

    /// Current image position.
    #[must_use]
    pub const fn position(&self) -> Camera2DPoint {
        self.position
    }

    /// Image center about which the transform is evaluated.
    #[must_use]
    pub const fn image_center(&self) -> Camera2DPoint {
        self.image_center
    }

    /// Transformed frame corners in their stored order.
    #[must_use]
    pub fn corners(&self) -> &[Camera2DPoint] {
        &self.corners
    }

    /// Five not-yet-named header words before the dimensions.
    #[must_use]
    pub const fn prefix_words(&self) -> [u32; 5] {
        self.prefix_words
    }

    /// Six not-yet-named header words after the center.
    #[must_use]
    pub const fn suffix_words(&self) -> [u32; 6] {
        self.suffix_words
    }

    /// Original transform payload for forward-compatible inspection.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// Validated 2D-camera layer metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera2DLayerData {
    layer_id: i64,
    canvas_id: i64,
    keyframes_enabled: bool,
    original_frame_center: Camera2DPoint,
    transform: Camera2DTransform,
}

impl Camera2DLayerData {
    /// Owning `Layer.MainId`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Owning canvas.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Whether timeline keyframe editing is enabled for the layer.
    #[must_use]
    pub const fn keyframes_enabled(&self) -> bool {
        self.keyframes_enabled
    }

    /// Original frame center stored in the layer row.
    #[must_use]
    pub const fn original_frame_center(&self) -> Camera2DPoint {
        self.original_frame_center
    }

    /// Current transform snapshot stored in the layer row.
    #[must_use]
    pub const fn transform(&self) -> &Camera2DTransform {
        &self.transform
    }
}

/// One named entry decoded from a track's inline `TrackValueMap`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTrackValueEntry {
    name: String,
    value: AnimationTrackValue,
}

impl AnimationTrackValueEntry {
    /// Parameter name, such as `ImageCelName`, `PlayTime`, or `AudioVolume`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Typed current/default value for the parameter.
    #[must_use]
    pub const fn value(&self) -> &AnimationTrackValue {
        &self.value
    }
}

/// One timeline track, its primary curves, and its inline value map.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTrack {
    id: i64,
    kind: AnimationTrackKind,
    layer_id: Option<i64>,
    next_track_id: Option<i64>,
    action_mixer_present: bool,
    secondary_action_mixer_present: bool,
    value_map_present: bool,
    values: Vec<AnimationTrackValueEntry>,
    curves: Vec<AnimationCurve>,
    secondary_curves: Vec<SecondaryAnimationCurve>,
    camera_2d_values: Option<Camera2DTrackValues>,
}

impl AnimationTrack {
    /// `Track.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Raw track kind with verified helpers for selected values.
    #[must_use]
    pub const fn kind(&self) -> AnimationTrackKind {
        self.kind
    }

    /// Layer matched through `LayerUuidWithTrack`, when present.
    #[must_use]
    pub const fn layer_id(&self) -> Option<i64> {
        self.layer_id
    }

    /// Next ID in the timeline's validated `TrackNextIndex` chain.
    #[must_use]
    pub const fn next_track_id(&self) -> Option<i64> {
        self.next_track_id
    }

    /// Whether `TrackActionMixer` was present.
    #[must_use]
    pub const fn action_mixer_present(&self) -> bool {
        self.action_mixer_present
    }

    /// Whether `TrackActionMixer2` contains an external-object identifier.
    #[must_use]
    pub const fn secondary_action_mixer_present(&self) -> bool {
        self.secondary_action_mixer_present
    }

    /// Whether the schema provided a non-NULL inline `TrackValueMap`.
    #[must_use]
    pub const fn value_map_present(&self) -> bool {
        self.value_map_present
    }

    /// Every validated entry from the inline `TrackValueMap`.
    #[must_use]
    pub fn values(&self) -> &[AnimationTrackValueEntry] {
        &self.values
    }

    /// Every validated `FCurve` in the primary action mixer.
    #[must_use]
    pub fn curves(&self) -> &[AnimationCurve] {
        &self.curves
    }

    /// Every validated double-precision value `FCurve` in the secondary mixer.
    ///
    /// These records are sparse, so this can be empty even when
    /// [`Self::secondary_action_mixer_present`] returns `true`.
    #[must_use]
    pub fn secondary_curves(&self) -> &[SecondaryAnimationCurve] {
        &self.secondary_curves
    }

    /// Values evaluated at the saved timeline position for a verified
    /// 2D-camera track.
    #[must_use]
    pub const fn camera_2d_values(&self) -> Option<Camera2DTrackValues> {
        self.camera_2d_values
    }
}

impl CelTrack {
    /// `Track.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Layer matched through `LayerUuidWithTrack`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Sorted cel-selection keys.
    #[must_use]
    pub fn keyframes(&self) -> &[CelKeyframe] {
        &self.keyframes
    }

    /// Returns the active cel tag at a display-frame position.
    #[must_use]
    pub fn cel_at_frame(&self, display_frame: f64, frame_rate: f64) -> Option<&str> {
        if !display_frame.is_finite() || !frame_rate.is_finite() || frame_rate <= 0.0 {
            return None;
        }
        let tick = display_frame * 60.0 / frame_rate;
        let index = self
            .keyframes
            .partition_point(|key| f64::from(key.time_60hz) <= tick + 1e-5)
            .checked_sub(1)?;
        self.keyframes.get(index).map(CelKeyframe::tag)
    }
}

/// One selected timeline and its cel-selection tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    timeline: Timeline,
    tracks: Vec<CelTrack>,
    animation_tracks: Vec<AnimationTrack>,
}

/// Result of cloning one animation track from an existing template.
///
/// The cloned row is appended to the requested timeline's track chain. Its
/// SQLite row identity, `MainId`, `TrackUuid`, and mixer external identifiers
/// are newly generated; unknown non-identity `Track` columns are copied from
/// the template.
#[cfg(feature = "write")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationTrackCloneSummary {
    template_track_id: i64,
    track_id: i64,
    timeline_id: i64,
    layer_id: i64,
    track_uuid: [u8; 16],
    primary_mixer_identifier: Option<Box<[u8]>>,
    secondary_mixer_identifier: Option<Box<[u8]>>,
}

#[cfg(feature = "write")]
impl AnimationTrackCloneSummary {
    /// Template `Track.MainId`.
    #[must_use]
    pub const fn template_track_id(&self) -> i64 {
        self.template_track_id
    }

    /// Newly allocated `Track.MainId`.
    #[must_use]
    pub const fn track_id(&self) -> i64 {
        self.track_id
    }

    /// Target `TimeLine.MainId`.
    #[must_use]
    pub const fn timeline_id(&self) -> i64 {
        self.timeline_id
    }

    /// Target `Layer.MainId`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Newly generated RFC 4122 variant, version-4 `TrackUuid` bytes.
    #[must_use]
    pub const fn track_uuid(&self) -> [u8; 16] {
        self.track_uuid
    }

    /// Newly allocated primary mixer external identifier, when present.
    #[must_use]
    pub fn primary_mixer_identifier(&self) -> Option<&[u8]> {
        self.primary_mixer_identifier.as_deref()
    }

    /// Newly allocated secondary mixer external identifier, when present.
    #[must_use]
    pub fn secondary_mixer_identifier(&self) -> Option<&[u8]> {
        self.secondary_mixer_identifier.as_deref()
    }
}

/// Result of unlinking and deleting one animation-track row.
///
/// Mixer external objects are intentionally retained in the container and its
/// index. This avoids deleting opaque data that may have non-Track references;
/// a later rewrite may reclaim such verified orphans separately.
#[cfg(feature = "write")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationTrackRemovalSummary {
    track_id: i64,
    timeline_id: i64,
    previous_track_id: Option<i64>,
    next_track_id: Option<i64>,
    retained_primary_mixer_identifier: Option<Box<[u8]>>,
    retained_secondary_mixer_identifier: Option<Box<[u8]>>,
}

#[cfg(feature = "write")]
impl AnimationTrackRemovalSummary {
    /// Removed `Track.MainId`.
    #[must_use]
    pub const fn track_id(&self) -> i64 {
        self.track_id
    }

    /// Timeline whose chain was repaired.
    #[must_use]
    pub const fn timeline_id(&self) -> i64 {
        self.timeline_id
    }

    /// Previous track in the repaired chain, or `None` for the first track.
    #[must_use]
    pub const fn previous_track_id(&self) -> Option<i64> {
        self.previous_track_id
    }

    /// Track following the removed row, when present.
    #[must_use]
    pub const fn next_track_id(&self) -> Option<i64> {
        self.next_track_id
    }

    /// Retained primary mixer identifier, when the row had one.
    #[must_use]
    pub fn retained_primary_mixer_identifier(&self) -> Option<&[u8]> {
        self.retained_primary_mixer_identifier.as_deref()
    }

    /// Retained secondary mixer identifier, when the row had one.
    #[must_use]
    pub fn retained_secondary_mixer_identifier(&self) -> Option<&[u8]> {
        self.retained_secondary_mixer_identifier.as_deref()
    }
}

impl Animation {
    /// Selected timeline.
    #[must_use]
    pub const fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Cel-selection tracks ordered by layer ID.
    #[must_use]
    pub fn tracks(&self) -> &[CelTrack] {
        &self.tracks
    }

    /// Finds the cel-selection track for a layer.
    #[must_use]
    pub fn track_for_layer(&self, layer_id: i64) -> Option<&CelTrack> {
        self.tracks
            .binary_search_by_key(&layer_id, CelTrack::layer_id)
            .ok()
            .map(|index| &self.tracks[index])
    }

    /// All timeline tracks ordered by `Track.MainId`.
    ///
    /// This includes tracks without a decoded `FCurve` and preserves unknown
    /// numeric track kinds.
    #[must_use]
    pub fn animation_tracks(&self) -> &[AnimationTrack] {
        &self.animation_tracks
    }

    /// Finds the verified 2D-camera track for a layer.
    #[must_use]
    pub fn camera_track_for_layer(&self, layer_id: i64) -> Option<&AnimationTrack> {
        self.animation_tracks
            .iter()
            .find(|track| track.layer_id == Some(layer_id) && track.kind.is_camera_2d())
    }
}

impl Database {
    /// Reads all timelines ordered by `MainId`.
    pub fn timelines(&self, limits: Limits) -> Result<Vec<Timeline>> {
        if self.schema().table("TimeLine").is_none() {
            return Ok(Vec::new());
        }
        for column in ["MainId", "BankId", "FrameRate", "StartFrame", "EndFrame"] {
            self.require_column("TimeLine", column)?;
        }
        let optional = |column: &'static str| {
            if self.schema().has_column("TimeLine", column) {
                column
            } else {
                "NULL"
            }
        };
        let sql = format!(
            "SELECT MainId, BankId, {}, {}, {}, FrameRate, StartFrame, EndFrame, {} \
             FROM TimeLine ORDER BY MainId",
            optional("NextTimeLine"),
            optional("FirstTrack"),
            optional("TimeLineName"),
            optional("CurrentFrame"),
        );
        let mut statement = self.connection().prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut timelines = Vec::new();
        let mut name_bytes = 0_u64;
        while let Some(row) = rows.next()? {
            enforce_item_limit(
                timelines.len() as u64 + 1,
                limits.max_animation_items(),
                "animation timelines",
            )?;
            let id: i64 = row.get(0)?;
            if id <= 0 {
                return Err(animation_error(format!(
                    "TimeLine.MainId must be positive, found {id}"
                )));
            }
            let frame_rate: f64 = row.get(5)?;
            let start_frame: f64 = row.get(6)?;
            let end_frame: f64 = row.get(7)?;
            let current_frame: Option<f64> = row.get(8)?;
            if !frame_rate.is_finite() || frame_rate <= 0.0 {
                return Err(animation_error(format!(
                    "timeline {id} has invalid frame rate {frame_rate}"
                )));
            }
            if !start_frame.is_finite()
                || !end_frame.is_finite()
                || start_frame > end_frame
                || current_frame.is_some_and(|value| !value.is_finite())
            {
                return Err(animation_error(format!(
                    "timeline {id} has an invalid frame range"
                )));
            }
            let name = optional_text(row.get_ref(4)?, 4, "TimeLineName")?;
            if let Some(value) = name {
                name_bytes = name_bytes
                    .checked_add(value.len() as u64)
                    .ok_or(Error::OffsetOverflow)?;
                if name_bytes > limits.max_animation_bytes() {
                    return Err(Error::LimitExceeded {
                        resource: "animation timeline names",
                        value: name_bytes,
                        limit: limits.max_animation_bytes(),
                    });
                }
            }
            timelines.push(Timeline {
                id,
                bank_id: row.get(1)?,
                next_timeline_id: row.get(2)?,
                first_track_id: row.get::<_, Option<i64>>(3)?.filter(|id| *id != 0),
                name: name.map(str::to_owned),
                frame_rate,
                start_frame,
                end_frame,
                current_frame,
            });
        }
        Ok(timelines)
    }

    /// Reads and validates 2D-camera metadata for one layer.
    ///
    /// `None` means the layer is absent, is not a 2D-camera layer, or the
    /// current schema predates the camera-specific columns.
    pub fn camera_2d_layer(
        &self,
        layer_id: i64,
        limits: Limits,
    ) -> Result<Option<Camera2DLayerData>> {
        let required = [
            "MainId",
            "CanvasId",
            "LayerType",
            "LayerFolder",
            "TimeLineLayerKeyFrameEnabled",
            "Camera2DResizableImageInfo",
            "Camera2DOriginalFrameCenterX",
            "Camera2DOriginalFrameCenterY",
        ];
        if required
            .iter()
            .any(|column| !self.schema().has_column("Layer", column))
        {
            return Ok(None);
        }
        let raw = self
            .connection()
            .query_row(
                "SELECT MainId, CanvasId, LayerType, LayerFolder, \
                 TimeLineLayerKeyFrameEnabled, Camera2DResizableImageInfo, \
                 Camera2DOriginalFrameCenterX, Camera2DOriginalFrameCenterY \
                 FROM Layer WHERE MainId = ?1 LIMIT 1",
                params![layer_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        optional_bytes(row.get_ref(5)?, 5, "Camera2DResizableImageInfo")?
                            .map(<[u8]>::to_vec),
                        row.get::<_, Option<f64>>(6)?,
                        row.get::<_, Option<f64>>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            layer_id,
            canvas_id,
            layer_type,
            folder_flags,
            keyframes_enabled,
            payload,
            original_x,
            original_y,
        )) = raw
        else {
            return Ok(None);
        };
        if layer_type & 512 == 0 {
            if payload.is_some() {
                return Err(animation_error(format!(
                    "layer {layer_id} has 2D-camera data without the camera layer bit"
                )));
            }
            return Ok(None);
        }
        if folder_flags == 0 {
            return Err(animation_error(format!(
                "2D-camera layer {layer_id} is not a folder"
            )));
        }
        if keyframes_enabled == 0 {
            return Err(animation_error(format!(
                "2D-camera layer {layer_id} does not have timeline keyframes enabled"
            )));
        }
        let payload = payload.ok_or_else(|| {
            animation_error(format!(
                "2D-camera layer {layer_id} has no transform snapshot"
            ))
        })?;
        enforce_byte_limit(
            payload.len() as u64,
            limits.max_animation_bytes(),
            "2D-camera transform snapshot",
        )?;
        let original_frame_center = Camera2DPoint {
            x: original_x.ok_or_else(|| animation_error("2D-camera original center X is NULL"))?,
            y: original_y.ok_or_else(|| animation_error("2D-camera original center Y is NULL"))?,
        };
        if !original_frame_center.x.is_finite() || !original_frame_center.y.is_finite() {
            return Err(animation_error(
                "2D-camera original frame center is not finite",
            ));
        }
        Ok(Some(Camera2DLayerData {
            layer_id,
            canvas_id,
            keyframes_enabled: true,
            original_frame_center,
            transform: parse_camera_2d_transform(&payload, limits)?,
        }))
    }
}

#[cfg(feature = "write")]
impl EditableDatabase {
    /// Replaces one existing typed current/default value in `TrackValueMap`.
    ///
    /// The named entry must be unique and its value kind must remain
    /// unchanged. Unknown value kinds are preserved while the map is rebuilt,
    /// but cannot be selected as the replacement target.
    pub fn replace_animation_track_value(
        &self,
        track_id: i64,
        name: &str,
        replacement: AnimationTrackValue,
        limits: Limits,
    ) -> Result<AnimationTrackValue> {
        replace_animation_track_value(
            self.connection(),
            self.schema(),
            track_id,
            name,
            replacement,
            limits,
        )
    }
}

#[cfg(feature = "write")]
fn replace_animation_track_value(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    track_id: i64,
    name: &str,
    replacement: AnimationTrackValue,
    limits: Limits,
) -> Result<AnimationTrackValue> {
    for column in ["MainId", "TrackValueMap"] {
        if !schema.has_column("Track", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Track.{column} is required to edit animation values"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Track WHERE MainId = ?1",
        params![track_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one animation track with ID {track_id}, found {row_count}"),
        });
    }
    let bytes = connection
        .query_row(
            "SELECT TrackValueMap FROM Track WHERE MainId = ?1 LIMIT 1",
            params![track_id],
            |row| {
                optional_bytes(row.get_ref(0)?, 0, "TrackValueMap")
                    .map(|value| value.map(<[u8]>::to_vec))
            },
        )
        .optional()?
        .flatten()
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation track {track_id} has no TrackValueMap"),
        })?;
    let mut entries = parse_track_value_map(&bytes, limits)?;
    let matching = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.name == name)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = matching.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation track {track_id} has {} values named {name:?}, expected exactly one",
                matching.len()
            ),
        });
    };
    validate_track_value_replacement(&entries[*index].value, &replacement)?;
    let original = std::mem::replace(&mut entries[*index].value, replacement);
    let encoded = encode_track_value_map(&entries, limits)?;
    let reparsed = parse_track_value_map(&encoded, limits)?;
    if reparsed != entries {
        return Err(Error::InvalidWrite {
            reason: "rebuilt TrackValueMap did not round-trip".to_owned(),
        });
    }
    if encoded == bytes {
        return Ok(original);
    }
    let changed = connection.execute(
        "UPDATE Track SET TrackValueMap = ?1 WHERE MainId = ?2",
        params![encoded, track_id],
    )?;
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("animation track {track_id} is not unique"),
        });
    }
    Ok(original)
}

#[cfg(feature = "write")]
fn validate_track_value_replacement(
    original: &AnimationTrackValue,
    replacement: &AnimationTrackValue,
) -> Result<()> {
    let valid = match (original, replacement) {
        (AnimationTrackValue::Float(_), AnimationTrackValue::Float(value)) => value.is_finite(),
        (AnimationTrackValue::IndexedText { .. }, AnimationTrackValue::IndexedText { .. }) => true,
        (AnimationTrackValue::Vector2 { .. }, AnimationTrackValue::Vector2 { x, y }) => {
            x.is_finite() && y.is_finite()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidWrite {
            reason: "animation value replacement must be finite, known, and keep its original kind"
                .to_owned(),
        })
    }
}

#[cfg(feature = "write")]
fn encode_track_value_map(entries: &[AnimationTrackValueEntry], limits: Limits) -> Result<Vec<u8>> {
    enforce_item_limit(
        entries.len() as u64,
        limits.max_animation_items(),
        "animation track values",
    )?;
    let count = u32::try_from(entries.len()).map_err(|_| Error::OffsetOverflow)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&8_u32.to_be_bytes());
    bytes.extend_from_slice(&count.to_be_bytes());
    for entry in entries {
        let start = bytes.len();
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        encode_utf16be_value(&mut bytes, &entry.name)?;
        match &entry.value {
            AnimationTrackValue::Float(value) => {
                if !value.is_finite() {
                    return Err(Error::InvalidWrite {
                        reason: "animation float replacement is not finite".to_owned(),
                    });
                }
                encode_utf16be_value(&mut bytes, "")?;
                bytes.extend_from_slice(&0_u32.to_be_bytes());
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            AnimationTrackValue::IndexedText {
                text,
                numeric_value,
            } => {
                encode_utf16be_value(&mut bytes, text)?;
                bytes.extend_from_slice(&2_u32.to_be_bytes());
                bytes.extend_from_slice(&numeric_value.to_be_bytes());
            }
            AnimationTrackValue::Vector2 { x, y } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err(Error::InvalidWrite {
                        reason: "animation vector replacement is not finite".to_owned(),
                    });
                }
                encode_utf16be_value(&mut bytes, "")?;
                bytes.extend_from_slice(&3_u32.to_be_bytes());
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
            }
            AnimationTrackValue::Unknown {
                kind,
                text,
                payload,
            } => {
                encode_utf16be_value(&mut bytes, text)?;
                bytes.extend_from_slice(&kind.to_be_bytes());
                bytes.extend_from_slice(payload);
            }
        }
        let size = u32::try_from(bytes.len() - start).map_err(|_| Error::OffsetOverflow)?;
        bytes[start..start + 4].copy_from_slice(&size.to_be_bytes());
        enforce_byte_limit(
            bytes.len() as u64,
            limits.max_animation_bytes(),
            "animation track value map",
        )?;
    }
    Ok(bytes)
}

#[cfg(feature = "write")]
fn encode_utf16be_value(bytes: &mut Vec<u8>, value: &str) -> Result<()> {
    let units = value.encode_utf16().collect::<Vec<_>>();
    let count = u32::try_from(units.len()).map_err(|_| Error::OffsetOverflow)?;
    bytes.extend_from_slice(&count.to_be_bytes());
    for unit in units {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Ok(())
}

impl<R: Read + Seek> ClipFile<R> {
    /// Reads the enabled timeline and its image-cel selection curves.
    ///
    /// Files without a `TimeLine` table or rows return `None`.
    pub fn read_animation(
        &mut self,
        database: &Database,
        limits: Limits,
    ) -> Result<Option<Animation>> {
        let timelines = database.timelines(limits)?;
        if timelines.is_empty() {
            return Ok(None);
        }
        let preferred = preferred_timeline_id(database)?;
        let timeline = if let Some(id) = preferred {
            timelines
                .iter()
                .find(|timeline| timeline.id == id)
                .cloned()
                .ok_or_else(|| {
                    animation_error(format!("AnimationCutBank references missing timeline {id}"))
                })?
        } else {
            timelines[0].clone()
        };
        Ok(Some(
            self.read_animation_timeline(database, timeline, limits)?,
        ))
    }

    /// Reads one timeline and all tracks in its validated chain.
    ///
    /// This is the explicit counterpart to [`Self::read_animation`], which
    /// selects the enabled timeline. An unknown ID or a file without timelines
    /// returns `None`.
    pub fn read_animation_for_timeline(
        &mut self,
        database: &Database,
        timeline_id: i64,
        limits: Limits,
    ) -> Result<Option<Animation>> {
        let timelines = database.timelines(limits)?;
        let Some(timeline) = timelines
            .into_iter()
            .find(|timeline| timeline.id == timeline_id)
        else {
            return Ok(None);
        };
        Ok(Some(
            self.read_animation_timeline(database, timeline, limits)?,
        ))
    }

    fn read_animation_timeline(
        &mut self,
        database: &Database,
        timeline: Timeline,
        limits: Limits,
    ) -> Result<Animation> {
        let sources = animation_track_sources(database, &timeline, limits)?;
        let layers = layer_uuid_ids(database, limits)?;
        let mut tracks = Vec::new();
        let mut animation_tracks = Vec::new();
        let mut total_curve_keys = 0_u64;
        let mut total_value_items = 0_u64;
        let mut total_value_bytes = 0_u64;
        for source in sources {
            let layer_id = match source.layer_uuid {
                Some(uuid) => Some(layers.get(&uuid).copied().ok_or_else(|| {
                    animation_error(format!(
                        "animation track {} has no matching layer UUID",
                        source.id
                    ))
                })?),
                None => None,
            };
            let curves = if let Some(identifier) = source.external_identifier.as_deref() {
                let object = self
                    .resolve_external_object(database, identifier)?
                    .ok_or_else(|| {
                        animation_error(format!(
                            "animation track {} references missing mixer data",
                            source.id
                        ))
                    })?;
                let ExternalBody::LengthPrefixedZlib(stream) = object.body() else {
                    return Err(animation_error(format!(
                        "animation track {} mixer is not a length-prefixed zlib stream",
                        source.id
                    )));
                };
                if stream.byte_order() != ByteOrder::LittleEndian {
                    return Err(animation_error(format!(
                        "animation track {} mixer uses an unexpected length byte order",
                        source.id
                    )));
                }
                let compressed =
                    self.read_length_prefixed_zlib(stream, limits.max_animation_bytes())?;
                let mixer = decompress_mixer(&compressed, limits.max_animation_bytes())?;
                let curves = parse_animation_curves(&mixer, limits)?;
                for curve in &curves {
                    total_curve_keys = total_curve_keys
                        .checked_add(curve.keyframes.len() as u64)
                        .ok_or(Error::OffsetOverflow)?;
                    enforce_item_limit(
                        total_curve_keys,
                        limits.max_animation_items(),
                        "animation curve keys",
                    )?;
                }
                curves
            } else {
                Vec::new()
            };
            let secondary_curves = if let Some(identifier) =
                source.secondary_external_identifier.as_deref()
            {
                let object = self
                    .resolve_external_object(database, identifier)?
                    .ok_or_else(|| {
                        animation_error(format!(
                            "animation track {} references missing secondary mixer data",
                            source.id
                        ))
                    })?;
                let ExternalBody::LengthPrefixedZlib(stream) = object.body() else {
                    return Err(animation_error(format!(
                        "animation track {} secondary mixer is not a length-prefixed zlib stream",
                        source.id
                    )));
                };
                if stream.byte_order() != ByteOrder::LittleEndian {
                    return Err(animation_error(format!(
                        "animation track {} secondary mixer uses an unexpected length byte order",
                        source.id
                    )));
                }
                let compressed =
                    self.read_length_prefixed_zlib(stream, limits.max_animation_bytes())?;
                let mixer = decompress_mixer(&compressed, limits.max_animation_bytes())?;
                let curves = parse_secondary_animation_curves(&mixer, limits)?;
                for curve in &curves {
                    total_curve_keys = total_curve_keys
                        .checked_add(curve.keyframes.len() as u64)
                        .ok_or(Error::OffsetOverflow)?;
                    enforce_item_limit(
                        total_curve_keys,
                        limits.max_animation_items(),
                        "animation curve keys",
                    )?;
                }
                curves
            } else {
                Vec::new()
            };
            let values = if let Some(value_map) = source.value_map.as_deref() {
                total_value_bytes = total_value_bytes
                    .checked_add(value_map.len() as u64)
                    .ok_or(Error::OffsetOverflow)?;
                enforce_byte_limit(
                    total_value_bytes,
                    limits.max_animation_bytes(),
                    "animation track value maps",
                )?;
                let values = parse_track_value_map(value_map, limits)?;
                total_value_items = total_value_items
                    .checked_add(values.len() as u64)
                    .ok_or(Error::OffsetOverflow)?;
                enforce_item_limit(
                    total_value_items,
                    limits.max_animation_items(),
                    "animation track values",
                )?;
                values
            } else {
                Vec::new()
            };
            let kind = AnimationTrackKind::new(source.kind);
            let camera_2d_values = if kind.is_camera_2d() {
                let camera_layer_id = layer_id.ok_or_else(|| {
                    animation_error(format!("2D-camera track {} has no layer UUID", source.id))
                })?;
                if database.camera_2d_layer(camera_layer_id, limits)?.is_none() {
                    return Err(animation_error(format!(
                        "2D-camera track {} does not resolve to camera-layer metadata",
                        source.id
                    )));
                }
                Some(parse_camera_2d_track_values(&values)?)
            } else {
                None
            };
            if kind.is_image_cel() {
                let layer_id = layer_id.ok_or_else(|| {
                    animation_error(format!("cel track {} has no layer UUID", source.id))
                })?;
                let curve = curves
                    .iter()
                    .find(|curve| curve.kind == "ImageCelName")
                    .ok_or_else(|| {
                        animation_error(format!(
                            "cel track {} mixer has no ImageCelName curve",
                            source.id
                        ))
                    })?;
                let keyframes = curve
                    .keyframes
                    .iter()
                    .map(|key| {
                        Ok(CelKeyframe {
                            time_60hz: key.time_60hz,
                            value: key.value,
                            tag: key.tag.clone().ok_or_else(|| {
                                animation_error(format!(
                                    "cel track {} ImageCelName key has no tag",
                                    source.id
                                ))
                            })?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                tracks.push(CelTrack {
                    id: source.id,
                    layer_id,
                    keyframes,
                });
            }
            animation_tracks.push(AnimationTrack {
                id: source.id,
                kind,
                layer_id,
                next_track_id: source.next_track_id,
                action_mixer_present: source.external_identifier.is_some(),
                secondary_action_mixer_present: source.secondary_external_identifier.is_some(),
                value_map_present: source.value_map.is_some(),
                values,
                curves,
                secondary_curves,
                camera_2d_values,
            });
        }
        tracks.sort_by_key(CelTrack::layer_id);
        if tracks
            .windows(2)
            .any(|pair| pair[0].layer_id == pair[1].layer_id)
        {
            return Err(animation_error(
                "multiple cel tracks resolve to the same layer",
            ));
        }
        Ok(Animation {
            timeline,
            tracks,
            animation_tracks,
        })
    }
}

#[cfg(feature = "write")]
impl<R: Read + Seek> ClipWriter<'_, R> {
    /// Clones an existing animation `Track` into another untracked layer.
    ///
    /// The template row supplies the complete known and unknown Track
    /// structure. The clone receives a newly allocated `MainId`, UUID-v4
    /// `TrackUuid`, and independent copies of both mixer external bodies, then
    /// is appended to `timeline_id`'s validated `TrackNextIndex` chain. The
    /// target layer must exist, have a valid UUID, and not already be linked
    /// from another Track.
    ///
    /// This API intentionally does not synthesize a mixer or infer a
    /// `TrackKind`: the BINC object metadata needed for arbitrary track
    /// construction is not fully understood. It also cannot prove that the
    /// template's raw `TrackKind` is semantically compatible with the target
    /// layer. Callers must choose the same kind of layer/template pair; using
    /// unrelated kinds can produce an application-incompatible file.
    pub fn clone_animation_track_from_template(
        &mut self,
        template_track_id: i64,
        timeline_id: i64,
        target_layer_id: i64,
        limits: Limits,
    ) -> Result<AnimationTrackCloneSummary> {
        validate_animation_track_clone_schema(self.database().schema())?;
        let update_elem_scheme = self.database().schema().table("ElemScheme").is_some();
        let source = writable_animation_track(
            self.database().connection(),
            self.database().schema(),
            template_track_id,
        )?;
        validate_unique_animation_mixers(
            self.database().connection(),
            self.database().schema(),
            template_track_id,
            &source,
        )?;

        let primary_body = clone_animation_mixer_body(
            self,
            template_track_id,
            "primary",
            source.primary_identifier.as_deref(),
            source.primary_size_column,
            source.primary_declared_size,
            limits,
        )?;
        let secondary_body = clone_animation_mixer_body(
            self,
            template_track_id,
            "secondary",
            source.secondary_identifier.as_deref(),
            source.secondary_size_column,
            source.secondary_declared_size,
            limits,
        )?;
        let (bank_id, previous_tail_id, track_count) =
            animation_clone_timeline_chain(self.database().connection(), timeline_id, limits)?;
        enforce_item_limit(
            track_count.checked_add(1).ok_or(Error::OffsetOverflow)?,
            limits.max_animation_items(),
            "animation tracks after clone",
        )?;
        let layer_uuid =
            animation_clone_layer_uuid(self.database().connection(), target_layer_id, limits)?;
        let track_id = next_animation_track_id(self.database().connection(), update_elem_scheme)?;
        let track_uuid = generate_animation_track_uuid(self.database().connection(), limits)?;
        let track_columns = animation_clone_columns(self.database().schema())?;

        let mut staged = Vec::<Vec<u8>>::new();
        let primary_identifier = match primary_body {
            Some(body) => {
                let identifier = self.stage_new_external_body(
                    body,
                    limits.max_animation_bytes(),
                    "cloned primary animation mixer body",
                )?;
                staged.push(identifier.clone());
                Some(identifier)
            }
            None => None,
        };
        let secondary_identifier = match secondary_body {
            Some(body) => match self.stage_new_external_body(
                body,
                limits.max_animation_bytes(),
                "cloned secondary animation mixer body",
            ) {
                Ok(identifier) => {
                    staged.push(identifier.clone());
                    Some(identifier)
                }
                Err(error) => {
                    rollback_animation_additions(self, &staged);
                    return Err(error);
                }
            },
            None => None,
        };

        let insertion = insert_animation_track_clone(
            self.database_mut().connection_mut(),
            &track_columns,
            AnimationTrackCloneInsert {
                template_track_id,
                track_id,
                timeline_id,
                bank_id,
                previous_tail_id,
                layer_uuid,
                track_uuid,
                update_elem_scheme,
                primary_identifier: primary_identifier.as_deref(),
                secondary_identifier: secondary_identifier.as_deref(),
                limits,
            },
        );
        if let Err(error) = insertion {
            rollback_animation_additions(self, &staged);
            return Err(error);
        }

        Ok(AnimationTrackCloneSummary {
            template_track_id,
            track_id,
            timeline_id,
            layer_id: target_layer_id,
            track_uuid,
            primary_mixer_identifier: primary_identifier.map(Box::from),
            secondary_mixer_identifier: secondary_identifier.map(Box::from),
        })
    }

    /// Clones and normalizes one verified image-cel track.
    ///
    /// The complete Track row and both opaque BINC object graphs still come
    /// from `template_track_id`; this method does not guess object metadata.
    /// It verifies that the template is kind `2000`, clones it with the normal
    /// UUID/chain/`ElemScheme` rules, then replaces its single
    /// `ImageCelName` curve with the exact requested non-empty key sequence.
    /// Any failure rolls back the cloned row and staged mixer bodies.
    pub fn clone_image_cel_track_from_template(
        &mut self,
        template_track_id: i64,
        timeline_id: i64,
        target_layer_id: i64,
        options: &ImageCelTrackCloneOptions,
        limits: Limits,
    ) -> Result<AnimationTrackCloneSummary> {
        validate_image_cel_clone_request(
            self.database().connection(),
            self.database().schema(),
            template_track_id,
            target_layer_id,
            options,
            limits,
        )?;
        let summary = self.clone_animation_track_from_template(
            template_track_id,
            timeline_id,
            target_layer_id,
            limits,
        )?;
        let result = normalize_cloned_image_cel_track(self, summary.track_id(), options, limits);
        if let Err(error) = result {
            if let Err(rollback) = rollback_cloned_animation_track(self, &summary) {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "image-cel clone normalization failed ({error}); rollback also failed ({rollback})"
                    ),
                });
            }
            return Err(error);
        }
        Ok(summary)
    }

    /// Inserts one complete key into an existing unique primary `FCurve`.
    ///
    /// All arrays already present in the curve are extended together. The
    /// matching secondary curve is updated in double precision when present.
    /// Unsupported or unrecognized per-key fields are rejected instead of
    /// synthesized. For `ImageCelName`, the typed current value is
    /// synchronized to the saved timeline position.
    pub fn insert_animation_curve_keyframe(
        &mut self,
        track_id: i64,
        curve_kind: &str,
        axis: Option<&str>,
        key_index: usize,
        insertion: &AnimationCurveKeyframeInsert,
        limits: Limits,
    ) -> Result<AnimationCurveKeyframe> {
        edit_animation_curve_keyframe(
            self,
            track_id,
            curve_kind,
            axis,
            CurveKeyEdit::Insert {
                index: key_index,
                key: insertion,
            },
            limits,
        )
    }

    /// Removes one key from an existing unique primary `FCurve`.
    ///
    /// The matching secondary curve and all of its per-key arrays are shortened
    /// together. Removing the final key is rejected because the empty-curve
    /// representation has not been proven for every track kind.
    pub fn remove_animation_curve_keyframe(
        &mut self,
        track_id: i64,
        curve_kind: &str,
        axis: Option<&str>,
        key_index: usize,
        limits: Limits,
    ) -> Result<AnimationCurveKeyframe> {
        edit_animation_curve_keyframe(
            self,
            track_id,
            curve_kind,
            axis,
            CurveKeyEdit::Remove { index: key_index },
            limits,
        )
    }

    /// Unlinks and deletes one existing Track row from a validated timeline.
    ///
    /// The timeline head or predecessor link is repaired transactionally.
    /// `ElemScheme.MaxIndex` remains a high-water mark. Mixer external objects
    /// and their index rows are retained conservatively.
    pub fn remove_animation_track(
        &mut self,
        timeline_id: i64,
        track_id: i64,
        limits: Limits,
    ) -> Result<AnimationTrackRemovalSummary> {
        validate_animation_track_removal_schema(self.database().schema())?;
        let source = writable_animation_track(
            self.database().connection(),
            self.database().schema(),
            track_id,
        )?;
        validate_unique_animation_mixers(
            self.database().connection(),
            self.database().schema(),
            track_id,
            &source,
        )?;
        remove_animation_track_row(
            self.database_mut().connection_mut(),
            timeline_id,
            track_id,
            source.primary_identifier,
            source.secondary_identifier,
            limits,
        )
    }

    /// Replaces the numeric time and value of one existing primary curve key.
    ///
    /// The matching primary `FCurve` must be unique. When the track has a
    /// matching secondary curve, its double-precision time and value are
    /// updated to the same values. Every unknown mixer byte and all other
    /// curve fields remain unchanged.
    pub fn replace_animation_curve_keyframe_numeric(
        &mut self,
        track_id: i64,
        curve_kind: &str,
        axis: Option<&str>,
        key_index: usize,
        replacement: AnimationCurveKeyframeValues,
        limits: Limits,
    ) -> Result<AnimationCurveKeyframe> {
        let time_60hz = replacement.time_60hz();
        let value = replacement.value();
        if !time_60hz.is_finite() || !value.is_finite() {
            return Err(Error::InvalidWrite {
                reason: "animation curve replacement must be finite".to_owned(),
            });
        }
        let source = writable_animation_track(
            self.database().connection(),
            self.database().schema(),
            track_id,
        )?;
        validate_unique_animation_mixers(
            self.database().connection(),
            self.database().schema(),
            track_id,
            &source,
        )?;
        let primary_id = source
            .primary_identifier
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("animation track {track_id} has no primary action mixer"),
            })?;
        let primary_body =
            self.external_body_for_update(&primary_id, limits.max_animation_bytes())?;
        let mut primary_mixer = decode_writable_mixer(&primary_body, limits)?;
        validate_declared_mixer_size(
            track_id,
            "primary",
            source.primary_size_column,
            source.primary_declared_size,
            primary_mixer.len(),
        )?;
        let primary_before = primary_mixer.clone();
        let original = patch_primary_curve_numeric(
            &mut primary_mixer,
            curve_kind,
            axis,
            key_index,
            time_60hz,
            value,
            limits,
        )?;
        let primary_replacement = encode_writable_mixer_for_writer(self, &primary_mixer, limits)?;

        let secondary_replacement = if let Some(identifier) = source.secondary_identifier.as_deref()
        {
            let body = self.external_body_for_update(identifier, limits.max_animation_bytes())?;
            let mut mixer = decode_writable_mixer(&body, limits)?;
            validate_declared_mixer_size(
                track_id,
                "secondary",
                source.secondary_size_column,
                source.secondary_declared_size,
                mixer.len(),
            )?;
            validate_matching_secondary_curve_keys(
                &primary_before,
                &mixer,
                curve_kind,
                axis,
                limits,
            )?;
            patch_secondary_curve_numeric(
                &mut mixer,
                curve_kind,
                axis,
                key_index,
                f64::from(time_60hz),
                f64::from(value),
                limits,
            )?;
            Some((
                identifier.to_vec(),
                encode_writable_mixer_for_writer(self, &mixer, limits)?,
            ))
        } else {
            None
        };

        let mut replacements = vec![(primary_id, primary_replacement)];
        if let Some((identifier, replacement)) = secondary_replacement {
            replacements.push((identifier, replacement));
        }
        install_animation_replacements(self, replacements)?;
        Ok(original)
    }

    /// Replaces one existing image-cel key's tag.
    ///
    /// Primary and matching secondary `ImageCelName` curves are updated
    /// together. If the track's typed current value names the same old cel, it
    /// is synchronized as well. This method does not add or remove keyframes.
    pub fn replace_animation_cel_tag(
        &mut self,
        track_id: i64,
        key_index: usize,
        tag: impl AsRef<str>,
        limits: Limits,
    ) -> Result<String> {
        let tag = tag.as_ref();
        if tag.len() > u8::MAX as usize {
            return Err(Error::InvalidWrite {
                reason: "animation mixer strings cannot exceed 255 UTF-8 bytes".to_owned(),
            });
        }
        let source = writable_animation_track(
            self.database().connection(),
            self.database().schema(),
            track_id,
        )?;
        validate_unique_animation_mixers(
            self.database().connection(),
            self.database().schema(),
            track_id,
            &source,
        )?;
        let primary_id = source
            .primary_identifier
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("animation track {track_id} has no primary action mixer"),
            })?;
        let primary_body =
            self.external_body_for_update(&primary_id, limits.max_animation_bytes())?;
        let mut primary_mixer = decode_writable_mixer(&primary_body, limits)?;
        validate_declared_mixer_size(
            track_id,
            "primary",
            source.primary_size_column,
            source.primary_declared_size,
            primary_mixer.len(),
        )?;
        let primary_before = primary_mixer.clone();
        let original = patch_primary_curve_tag(&mut primary_mixer, key_index, tag, limits)?;
        let primary_size = primary_mixer.len();
        let primary_replacement = encode_writable_mixer_for_writer(self, &primary_mixer, limits)?;

        let secondary_replacement = if let Some(identifier) = source.secondary_identifier.as_deref()
        {
            let body = self.external_body_for_update(identifier, limits.max_animation_bytes())?;
            let mut mixer = decode_writable_mixer(&body, limits)?;
            validate_declared_mixer_size(
                track_id,
                "secondary",
                source.secondary_size_column,
                source.secondary_declared_size,
                mixer.len(),
            )?;
            if !validate_matching_secondary_curve_keys(
                &primary_before,
                &mixer,
                "ImageCelName",
                None,
                limits,
            )? {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "animation track {track_id} has no matching secondary ImageCelName curve"
                    ),
                });
            }
            if let Some(previous) = patch_secondary_curve_tag(&mut mixer, key_index, tag, limits)? {
                if previous != original {
                    return Err(Error::InvalidWrite {
                        reason: format!(
                            "animation track {track_id} has inconsistent primary and secondary cel tags"
                        ),
                    });
                }
                let size = mixer.len();
                Some((
                    identifier.to_vec(),
                    encode_writable_mixer_for_writer(self, &mixer, limits)?,
                    size,
                ))
            } else {
                None
            }
        } else {
            None
        };

        let updated_value_map = source
            .value_map
            .as_deref()
            .map(|bytes| synchronize_cel_track_value(bytes, &original, tag, limits))
            .transpose()?
            .flatten();

        let mut replacements = vec![(primary_id, primary_replacement)];
        if let Some((identifier, replacement, _)) = &secondary_replacement {
            replacements.push((identifier.clone(), replacement.clone()));
        }
        let installed = install_animation_replacements(self, replacements)?;
        let secondary_size = secondary_replacement
            .as_ref()
            .map(|(_, _, size)| *size)
            .or_else(|| {
                source
                    .secondary_declared_size
                    .and_then(|size| usize::try_from(size).ok())
            });
        if let Err(error) = update_animation_track_metadata(
            self.database().connection(),
            track_id,
            source.primary_size_column.then_some(primary_size),
            source
                .secondary_size_column
                .then_some(secondary_size)
                .flatten(),
            updated_value_map.as_deref(),
        ) {
            rollback_animation_replacements(self, installed);
            return Err(error);
        }
        Ok(original)
    }
}

#[cfg(feature = "write")]
fn validate_image_cel_clone_request(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    template_track_id: i64,
    target_layer_id: i64,
    options: &ImageCelTrackCloneOptions,
    limits: Limits,
) -> Result<()> {
    for column in [
        "MainId",
        "LayerType",
        "LayerFolder",
        "AnimationFolder",
        "LayerFirstChildIndex",
        "LayerNextIndex",
        "LayerName",
    ] {
        if !schema.has_column("Layer", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Layer.{column} is required to validate an image-cel track target"),
            });
        }
    }
    let kind: Option<i64> = connection
        .query_row(
            "SELECT TrackKind FROM Track WHERE MainId = ?1",
            params![template_track_id],
            |row| row.get(0),
        )
        .optional()?;
    if kind != Some(2000) {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation template {template_track_id} is not a verified image-cel track"
            ),
        });
    }
    enforce_item_limit(
        options.keyframes.len() as u64,
        limits.max_animation_items(),
        "image-cel track clone keys",
    )?;
    if options.keyframes.is_empty() {
        return Err(Error::InvalidWrite {
            reason: "image-cel track clone requires at least one key".to_owned(),
        });
    }
    if options
        .keyframes
        .windows(2)
        .any(|pair| pair[0].time_60hz > pair[1].time_60hz)
    {
        return Err(Error::InvalidWrite {
            reason: "image-cel track clone keys must be sorted by 60 Hz time".to_owned(),
        });
    }
    let mut by_tag = BTreeMap::<&str, u32>::new();
    let mut by_value = BTreeMap::<u32, &str>::new();
    let mut tag_bytes = 0_u64;
    for key in &options.keyframes {
        if !key.time_60hz.is_finite() {
            return Err(Error::InvalidWrite {
                reason: "image-cel track clone key time must be finite".to_owned(),
            });
        }
        if !u32_is_exactly_representable_as_f32(key.numeric_value) {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "image-cel numeric value {} is not exactly representable as f32",
                    key.numeric_value
                ),
            });
        }
        if key.tag.len() > u8::MAX as usize {
            return Err(Error::InvalidWrite {
                reason: "image-cel tags cannot exceed 255 UTF-8 bytes".to_owned(),
            });
        }
        tag_bytes = tag_bytes
            .checked_add(key.tag.len() as u64)
            .ok_or(Error::OffsetOverflow)?;
        enforce_byte_limit(
            tag_bytes,
            limits.max_animation_bytes(),
            "image-cel track clone tags",
        )?;
        if by_tag
            .insert(&key.tag, key.numeric_value)
            .is_some_and(|previous| previous != key.numeric_value)
            || by_value
                .insert(key.numeric_value, &key.tag)
                .is_some_and(|previous| previous != key.tag)
        {
            return Err(Error::InvalidWrite {
                reason: "image-cel clone must map each tag to exactly one numeric value".to_owned(),
            });
        }
    }

    let (layer_type, folder, animation_folder, first_child): (i64, i64, i64, Option<i64>) =
        connection
            .query_row(
                "SELECT LayerType, LayerFolder, AnimationFolder, LayerFirstChildIndex \
             FROM Layer WHERE MainId = ?1",
                params![target_layer_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        nonzero_track_id(row.get(3)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("target image-cel layer {target_layer_id} does not exist"),
            })?;
    if layer_type != 0 || folder == 0 || animation_folder != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "target image-cel layer {target_layer_id} must be an observed type-0 animation folder"
            ),
        });
    }
    let mut child = first_child.ok_or_else(|| Error::InvalidWrite {
        reason: format!("target image-cel layer {target_layer_id} has no child cels"),
    })?;
    let mut child_names = BTreeMap::<String, ()>::new();
    let mut visited = BTreeMap::<i64, ()>::new();
    loop {
        if visited.insert(child, ()).is_some() {
            return Err(Error::InvalidWrite {
                reason: "target image-cel layer has a cyclic child chain".to_owned(),
            });
        }
        enforce_item_limit(
            visited.len() as u64,
            limits.max_animation_items(),
            "target image-cel child layers",
        )?;
        let (name, next): (String, Option<i64>) = connection
            .query_row(
                "SELECT LayerName, LayerNextIndex FROM Layer WHERE MainId = ?1",
                params![child],
                |row| {
                    let name = optional_text(row.get_ref(0)?, 0, "LayerName")?
                        .ok_or_else(|| {
                            rusqlite::Error::InvalidColumnType(
                                0,
                                "LayerName".to_owned(),
                                rusqlite::types::Type::Null,
                            )
                        })?
                        .to_owned();
                    Ok((name, nonzero_track_id(row.get(1)?)))
                },
            )
            .optional()?
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("target image-cel folder references missing child layer {child}"),
            })?;
        if child_names.insert(name.clone(), ()).is_some() {
            return Err(Error::InvalidWrite {
                reason: format!("target image-cel folder repeats child name {name:?}"),
            });
        }
        let Some(next) = next else {
            break;
        };
        child = next;
    }
    for key in &options.keyframes {
        if !child_names.contains_key(&key.tag) {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "image-cel key tag {:?} does not name an immediate target child layer",
                    key.tag
                ),
            });
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
fn u32_is_exactly_representable_as_f32(value: u32) -> bool {
    f64::from(value as f32) == f64::from(value)
}

#[cfg(feature = "write")]
fn normalize_cloned_image_cel_track<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    track_id: i64,
    options: &ImageCelTrackCloneOptions,
    limits: Limits,
) -> Result<()> {
    let source = writable_animation_track(
        writer.database().connection(),
        writer.database().schema(),
        track_id,
    )?;
    let identifier = source
        .primary_identifier
        .as_deref()
        .ok_or_else(|| Error::InvalidWrite {
            reason: "cloned image-cel track has no primary action mixer".to_owned(),
        })?;
    let body = writer.external_body_for_update(identifier, limits.max_animation_bytes())?;
    let mixer = decode_writable_mixer(&body, limits)?;
    let matching = parse_animation_curves(&mixer, limits)?
        .into_iter()
        .filter(|curve| curve.kind == "ImageCelName" && curve.axis.is_none())
        .collect::<Vec<_>>();
    let [curve] = matching.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: format!(
                "image-cel template has {} ImageCelName curves, expected exactly one",
                matching.len()
            ),
        });
    };
    for index in (1..curve.keyframes.len()).rev() {
        writer.remove_animation_curve_keyframe(track_id, "ImageCelName", None, index, limits)?;
    }
    let first = &options.keyframes[0];
    writer.replace_animation_curve_keyframe_numeric(
        track_id,
        "ImageCelName",
        None,
        0,
        AnimationCurveKeyframeValues::new(first.time_60hz, first.numeric_value as f32),
        limits,
    )?;
    writer.replace_animation_cel_tag(track_id, 0, &first.tag, limits)?;
    for (index, key) in options.keyframes.iter().enumerate().skip(1) {
        let insertion = AnimationCurveKeyframeInsert::new(key.time_60hz, key.numeric_value as f32)
            .with_tag(&key.tag)
            .with_interpolation("Constant")
            .with_slopes(0.0, 0.0)
            .with_revise_constant(1);
        writer.insert_animation_curve_keyframe(
            track_id,
            "ImageCelName",
            None,
            index,
            &insertion,
            limits,
        )?;
    }
    synchronize_image_cel_value_map(writer, track_id, limits)
}

#[cfg(feature = "write")]
fn synchronize_image_cel_value_map<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    track_id: i64,
    limits: Limits,
) -> Result<()> {
    let source = writable_animation_track(
        writer.database().connection(),
        writer.database().schema(),
        track_id,
    )?;
    let identifier = source
        .primary_identifier
        .as_deref()
        .ok_or_else(|| Error::InvalidWrite {
            reason: "image-cel track has no primary action mixer".to_owned(),
        })?;
    let body = writer.external_body_for_update(identifier, limits.max_animation_bytes())?;
    let mixer = decode_writable_mixer(&body, limits)?;
    let matching = parse_animation_curves(&mixer, limits)?
        .into_iter()
        .filter(|curve| curve.kind == "ImageCelName" && curve.axis.is_none())
        .collect::<Vec<_>>();
    let [curve] = matching.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: "image-cel track must contain one ImageCelName curve".to_owned(),
        });
    };
    let value_map = source
        .value_map
        .as_deref()
        .ok_or_else(|| Error::InvalidWrite {
            reason: "image-cel track has no TrackValueMap".to_owned(),
        })?;
    let bank_id = source.bank_id.ok_or_else(|| Error::InvalidWrite {
        reason: "image-cel Track.BankId is required for current-value synchronization".to_owned(),
    })?;
    let tick = saved_timeline_tick(writer.database().connection(), bank_id)?;
    let encoded = synchronize_cel_track_value_at_tick(value_map, &curve.keyframes, tick, limits)?;
    update_animation_track_metadata(
        writer.database().connection(),
        track_id,
        None,
        None,
        Some(&encoded),
    )
}

#[cfg(feature = "write")]
fn rollback_cloned_animation_track<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    summary: &AnimationTrackCloneSummary,
) -> Result<()> {
    let track_id = summary.track_id;
    let timeline_id = summary.timeline_id;
    let connection = writer.database_mut().connection_mut();
    (|| -> Result<()> {
        let transaction = connection.transaction()?;
        let predecessor: Option<i64> = transaction
            .query_row(
                "SELECT MainId FROM Track WHERE TrackNextIndex = ?1",
                params![track_id],
                |row| row.get(0),
            )
            .optional()?;
        let changed = if let Some(predecessor) = predecessor {
            transaction.execute(
                "UPDATE Track SET TrackNextIndex = 0 WHERE MainId = ?1 AND TrackNextIndex = ?2",
                params![predecessor, track_id],
            )?
        } else {
            transaction.execute(
                "UPDATE TimeLine SET FirstTrack = 0 WHERE MainId = ?1 AND FirstTrack = ?2",
                params![timeline_id, track_id],
            )?
        };
        if changed != 1 {
            return Err(Error::InvalidWrite {
                reason: "failed to unlink a rolled-back image-cel track clone".to_owned(),
            });
        }
        let deleted =
            transaction.execute("DELETE FROM Track WHERE MainId = ?1", params![track_id])?;
        if deleted != 1 {
            return Err(Error::InvalidWrite {
                reason: "failed to delete a rolled-back image-cel track clone".to_owned(),
            });
        }
        if transaction.query_row(
            "SELECT count(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'ElemScheme'",
            [],
            |row| row.get::<_, i64>(0),
        )? == 1
        {
            let restored = transaction.execute(
                "UPDATE ElemScheme SET MaxIndex = ?1 \
                 WHERE TableName = 'Track' AND MaxIndex = ?2",
                params![track_id - 1, track_id],
            )?;
            if restored != 1 {
                return Err(Error::InvalidWrite {
                    reason: "failed to restore ElemScheme after image-cel clone rollback"
                        .to_owned(),
                });
            }
        }
        transaction.commit()?;
        Ok(())
    })()?;
    for identifier in [
        summary.secondary_mixer_identifier(),
        summary.primary_mixer_identifier(),
    ]
    .into_iter()
    .flatten()
    {
        let removed = writer.unstage_new_external_body(identifier);
        if removed.is_none() {
            return Err(Error::InvalidWrite {
                reason: "rolled-back image-cel clone mixer was not staged".to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
fn validate_animation_track_removal_schema(schema: &DatabaseSchema) -> Result<()> {
    for (table, column) in [
        ("Track", "MainId"),
        ("Track", "BankId"),
        ("Track", "TrackNextIndex"),
        ("Track", "TrackActionMixer"),
        ("TimeLine", "MainId"),
        ("TimeLine", "BankId"),
        ("TimeLine", "FirstTrack"),
    ] {
        if !schema.has_column(table, column) {
            return Err(Error::InvalidWrite {
                reason: format!("{table}.{column} is required to remove an animation track"),
            });
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
fn remove_animation_track_row(
    connection: &mut rusqlite::Connection,
    timeline_id: i64,
    track_id: i64,
    primary_identifier: Option<Vec<u8>>,
    secondary_identifier: Option<Vec<u8>>,
    limits: Limits,
) -> Result<AnimationTrackRemovalSummary> {
    let (timeline_bank, first_track): (i64, Option<i64>) = connection
        .query_row(
            "SELECT BankId, FirstTrack FROM TimeLine WHERE MainId = ?1",
            params![timeline_id],
            |row| Ok((row.get(0)?, nonzero_track_id(row.get(1)?))),
        )
        .optional()?
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("target timeline {timeline_id} does not exist"),
        })?;
    let (track_bank, next_track): (i64, Option<i64>) = connection
        .query_row(
            "SELECT BankId, TrackNextIndex FROM Track WHERE MainId = ?1",
            params![track_id],
            |row| Ok((row.get(0)?, nonzero_track_id(row.get(1)?))),
        )
        .optional()?
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation track {track_id} does not exist"),
        })?;
    if track_bank != timeline_bank {
        return Err(Error::InvalidWrite {
            reason: format!("animation track {track_id} does not belong to timeline {timeline_id}"),
        });
    }
    animation_clone_timeline_chain(connection, timeline_id, limits)?;
    let previous_track_id = if first_track == Some(track_id) {
        None
    } else {
        let mut statement = connection
            .prepare("SELECT MainId FROM Track WHERE BankId = ?1 AND TrackNextIndex = ?2")?;
        let predecessors = statement
            .query_map(params![timeline_bank, track_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let [previous] = predecessors.as_slice() else {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation track {track_id} has {} chain predecessors",
                    predecessors.len()
                ),
            });
        };
        Some(*previous)
    };
    let transaction = connection.transaction()?;
    let replacement = next_track.unwrap_or(0);
    let relinked = if let Some(previous) = previous_track_id {
        transaction.execute(
            "UPDATE Track SET TrackNextIndex = ?1 \
             WHERE MainId = ?2 AND BankId = ?3 AND TrackNextIndex = ?4",
            params![replacement, previous, timeline_bank, track_id],
        )?
    } else {
        transaction.execute(
            "UPDATE TimeLine SET FirstTrack = ?1 \
             WHERE MainId = ?2 AND FirstTrack = ?3",
            params![replacement, timeline_id, track_id],
        )?
    };
    if relinked != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("animation track {track_id} chain repair affected {relinked} rows"),
        });
    }
    let deleted = transaction.execute(
        "DELETE FROM Track WHERE MainId = ?1 AND BankId = ?2",
        params![track_id, timeline_bank],
    )?;
    if deleted != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("animation track {track_id} deletion affected {deleted} rows"),
        });
    }
    animation_clone_timeline_chain(&transaction, timeline_id, limits)?;
    transaction.commit()?;
    Ok(AnimationTrackRemovalSummary {
        track_id,
        timeline_id,
        previous_track_id,
        next_track_id: next_track,
        retained_primary_mixer_identifier: primary_identifier.map(Box::from),
        retained_secondary_mixer_identifier: secondary_identifier.map(Box::from),
    })
}

#[cfg(feature = "write")]
fn validate_animation_track_clone_schema(schema: &DatabaseSchema) -> Result<()> {
    for (table, columns) in [
        (
            "Track",
            &[
                "MainId",
                "BankId",
                "TrackNextIndex",
                "TrackActionMixer",
                "TrackUuid",
                "LayerUuidWithTrack",
            ][..],
        ),
        ("TimeLine", &["MainId", "BankId", "FirstTrack"][..]),
        ("Layer", &["MainId", "LayerUuid"][..]),
    ] {
        for column in columns {
            if !schema.has_column(table, column) {
                return Err(Error::InvalidWrite {
                    reason: format!("{table}.{column} is required to clone an animation track"),
                });
            }
        }
    }
    if schema.table("ElemScheme").is_some() {
        for column in ["TableName", "MaxIndex"] {
            if !schema.has_column("ElemScheme", column) {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "ElemScheme.{column} is required when cloning an animation track"
                    ),
                });
            }
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
fn clone_animation_mixer_body<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    track_id: i64,
    label: &'static str,
    identifier: Option<&[u8]>,
    size_column: bool,
    declared_size: Option<i64>,
    limits: Limits,
) -> Result<Option<Vec<u8>>> {
    let Some(identifier) = identifier else {
        if size_column && declared_size.is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation track {track_id} has a {label} mixer size but no identifier"
                ),
            });
        }
        return Ok(None);
    };
    let body = writer.external_body_for_update(identifier, limits.max_animation_bytes())?;
    let mixer = decode_writable_mixer(&body, limits)?;
    validate_declared_mixer_size(track_id, label, size_column, declared_size, mixer.len())?;
    Ok(Some(body))
}

#[cfg(feature = "write")]
fn animation_clone_timeline_chain(
    connection: &rusqlite::Connection,
    timeline_id: i64,
    limits: Limits,
) -> Result<(i64, Option<i64>, u64)> {
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM TimeLine WHERE MainId = ?1",
        params![timeline_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "expected one target timeline with ID {timeline_id}, found {row_count}"
            ),
        });
    }
    let (bank_id, first_track_id) = connection.query_row(
        "SELECT BankId, FirstTrack FROM TimeLine WHERE MainId = ?1 LIMIT 1",
        params![timeline_id],
        |row| Ok((row.get::<_, i64>(0)?, nonzero_track_id(row.get(1)?))),
    )?;

    let mut statement = connection
        .prepare("SELECT MainId, TrackNextIndex FROM Track WHERE BankId = ?1 ORDER BY MainId")?;
    let mut rows = statement.query(params![bank_id])?;
    let mut by_id = BTreeMap::<i64, Option<i64>>::new();
    while let Some(row) = rows.next()? {
        enforce_item_limit(
            by_id.len() as u64 + 1,
            limits.max_animation_items(),
            "animation timeline tracks",
        )?;
        let id: i64 = row.get(0)?;
        let next = nonzero_track_id(row.get(1)?);
        if by_id.insert(id, next).is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "target timeline {timeline_id} contains duplicate Track.MainId {id}"
                ),
            });
        }
    }
    let Some(mut current) = first_track_id else {
        if !by_id.is_empty() {
            return Err(Error::InvalidWrite {
                reason: format!("target timeline {timeline_id} has tracks but no FirstTrack"),
            });
        }
        return Ok((bank_id, None, 0));
    };
    let mut visited = BTreeMap::<i64, ()>::new();
    loop {
        if visited.insert(current, ()).is_some() {
            return Err(Error::InvalidWrite {
                reason: format!("target timeline {timeline_id} track chain is cyclic at {current}"),
            });
        }
        enforce_item_limit(
            visited.len() as u64,
            limits.max_animation_items(),
            "animation timeline track chain",
        )?;
        let next = by_id.get(&current).ok_or_else(|| Error::InvalidWrite {
            reason: format!("target timeline {timeline_id} references missing track {current}"),
        })?;
        match next {
            Some(next) => current = *next,
            None => break,
        }
    }
    if visited.len() != by_id.len() {
        return Err(Error::InvalidWrite {
            reason: format!("target timeline {timeline_id} track chain contains unreachable rows"),
        });
    }
    Ok((bank_id, Some(current), visited.len() as u64))
}

#[cfg(feature = "write")]
fn animation_clone_layer_uuid(
    connection: &rusqlite::Connection,
    layer_id: i64,
    limits: Limits,
) -> Result<[u8; 16]> {
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Layer WHERE MainId = ?1",
        params![layer_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one target layer with ID {layer_id}, found {row_count}"),
        });
    }
    let raw = connection.query_row(
        "SELECT LayerUuid FROM Layer WHERE MainId = ?1 LIMIT 1",
        params![layer_id],
        |row| {
            optional_bytes(row.get_ref(0)?, 0, "LayerUuid")?
                .map(<[u8]>::to_vec)
                .ok_or_else(|| {
                    rusqlite::Error::InvalidColumnType(
                        0,
                        "LayerUuid".to_owned(),
                        rusqlite::types::Type::Null,
                    )
                })
        },
    )?;
    enforce_byte_limit(
        raw.len() as u64,
        limits.max_identifier_size(),
        "target animation layer UUID",
    )?;
    let uuid = normalize_uuid(&raw)?;

    let mut statement =
        connection.prepare("SELECT MainId, LayerUuid FROM Layer WHERE LayerUuid IS NOT NULL")?;
    let mut rows = statement.query([])?;
    let mut matching_layers = Vec::new();
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        count = count.checked_add(1).ok_or(Error::OffsetOverflow)?;
        enforce_item_limit(count, limits.max_animation_items(), "animation layer UUIDs")?;
        let value = required_bytes(row.get_ref(1)?, 1, "LayerUuid")?;
        enforce_byte_limit(
            value.len() as u64,
            limits.max_identifier_size(),
            "animation layer UUID",
        )?;
        if normalize_uuid(value)? == uuid {
            matching_layers.push(row.get::<_, i64>(0)?);
        }
    }
    if matching_layers != [layer_id] {
        return Err(Error::InvalidWrite {
            reason: format!(
                "target layer {layer_id} UUID resolves to {} Layer rows",
                matching_layers.len()
            ),
        });
    }

    let mut statement = connection
        .prepare("SELECT LayerUuidWithTrack FROM Track WHERE LayerUuidWithTrack IS NOT NULL")?;
    let mut rows = statement.query([])?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        count = count.checked_add(1).ok_or(Error::OffsetOverflow)?;
        enforce_item_limit(
            count,
            limits.max_animation_items(),
            "animation layer associations",
        )?;
        let value = required_bytes(row.get_ref(0)?, 0, "LayerUuidWithTrack")?;
        enforce_byte_limit(
            value.len() as u64,
            limits.max_identifier_size(),
            "animation layer UUID",
        )?;
        if normalize_uuid(value)? == uuid {
            return Err(Error::InvalidWrite {
                reason: format!("target layer {layer_id} already has an animation track"),
            });
        }
    }
    Ok(uuid)
}

#[cfg(feature = "write")]
fn next_animation_track_id(
    connection: &rusqlite::Connection,
    use_elem_scheme: bool,
) -> Result<i64> {
    let (row_count, non_null_count, distinct_count): (i64, i64, i64) = connection.query_row(
        "SELECT count(*), count(MainId), count(DISTINCT MainId) FROM Track",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let null_count = row_count - non_null_count;
    if null_count != 0 {
        return Err(Error::InvalidWrite {
            reason: "Track contains NULL MainId values".to_owned(),
        });
    }
    if distinct_count != row_count {
        return Err(Error::InvalidWrite {
            reason: "Track contains duplicate MainId values".to_owned(),
        });
    }
    let maximum: Option<i64> =
        connection.query_row("SELECT max(MainId) FROM Track", [], |row| row.get(0))?;
    let maximum = maximum.unwrap_or(0);
    let allocation_base = if use_elem_scheme {
        let (row_count, max_index): (i64, Option<i64>) = connection.query_row(
            "SELECT count(*), max(MaxIndex) FROM ElemScheme WHERE TableName = 'Track'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if row_count != 1 {
            return Err(Error::InvalidWrite {
                reason: format!("ElemScheme must contain exactly one Track row, found {row_count}"),
            });
        }
        let max_index = max_index.ok_or_else(|| Error::InvalidWrite {
            reason: "ElemScheme Track MaxIndex is NULL".to_owned(),
        })?;
        if max_index < maximum {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "ElemScheme Track MaxIndex {max_index} is below Track.MainId maximum {maximum}"
                ),
            });
        }
        max_index
    } else {
        maximum
    };
    let track_id = allocation_base
        .checked_add(1)
        .ok_or(Error::OffsetOverflow)?;
    if track_id <= 0 {
        return Err(Error::InvalidWrite {
            reason: "could not allocate a positive Track.MainId".to_owned(),
        });
    }
    Ok(track_id)
}

#[cfg(feature = "write")]
fn generate_animation_track_uuid(
    connection: &rusqlite::Connection,
    limits: Limits,
) -> Result<[u8; 16]> {
    let mut occupied = BTreeMap::<[u8; 16], ()>::new();
    for (table, column) in [
        ("Track", "TrackUuid"),
        ("Track", "LayerUuidWithTrack"),
        ("Layer", "LayerUuid"),
    ] {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} IS NOT NULL",
            quote_sql_identifier(column),
            quote_sql_identifier(table),
            quote_sql_identifier(column),
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut count = 0_u64;
        while let Some(row) = rows.next()? {
            count = count.checked_add(1).ok_or(Error::OffsetOverflow)?;
            enforce_item_limit(
                count,
                limits.max_animation_items(),
                "animation UUID candidates",
            )?;
            let value = required_bytes(row.get_ref(0)?, 0, column)?;
            enforce_byte_limit(
                value.len() as u64,
                limits.max_identifier_size(),
                "animation UUID",
            )?;
            let uuid = normalize_uuid(value)?;
            if table == "Track" && column == "TrackUuid" && occupied.insert(uuid, ()).is_some() {
                return Err(Error::InvalidWrite {
                    reason: "Track contains duplicate TrackUuid values".to_owned(),
                });
            }
            occupied.insert(uuid, ());
        }
    }
    for _ in 0..128 {
        let mut random: Vec<u8> =
            connection.query_row("SELECT randomblob(16)", [], |row| row.get(0))?;
        if random.len() != 16 {
            return Err(Error::InvalidWrite {
                reason: "SQLite returned an invalid animation UUID seed".to_owned(),
            });
        }
        random[6] = (random[6] & 0x0f) | 0x40;
        random[8] = (random[8] & 0x3f) | 0x80;
        let uuid: [u8; 16] = random
            .try_into()
            .expect("SQLite random seed length checked above");
        if !occupied.contains_key(&uuid) {
            return Ok(uuid);
        }
    }
    Err(Error::InvalidWrite {
        reason: "could not generate a unique animation TrackUuid".to_owned(),
    })
}

#[cfg(feature = "write")]
fn animation_clone_columns(schema: &DatabaseSchema) -> Result<Vec<String>> {
    let table = schema.table("Track").ok_or_else(|| Error::InvalidWrite {
        reason: "Track table is required to clone an animation track".to_owned(),
    })?;
    let mut columns = Vec::new();
    for column in table.columns() {
        if column.name() == "_PW_ID" {
            if column.primary_key_position() == 0 {
                return Err(Error::InvalidWrite {
                    reason: "Track._PW_ID is not a regeneratable primary key".to_owned(),
                });
            }
            continue;
        }
        if column.primary_key_position() != 0 {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "cannot safely regenerate unexpected Track primary-key column {:?}",
                    column.name()
                ),
            });
        }
        if column.hidden() == 0 {
            columns.push(column.name().to_owned());
        }
    }
    if columns.is_empty() {
        return Err(Error::InvalidWrite {
            reason: "Track has no cloneable columns".to_owned(),
        });
    }
    Ok(columns)
}

#[cfg(feature = "write")]
struct AnimationTrackCloneInsert<'a> {
    template_track_id: i64,
    track_id: i64,
    timeline_id: i64,
    bank_id: i64,
    previous_tail_id: Option<i64>,
    layer_uuid: [u8; 16],
    track_uuid: [u8; 16],
    update_elem_scheme: bool,
    primary_identifier: Option<&'a [u8]>,
    secondary_identifier: Option<&'a [u8]>,
    limits: Limits,
}

#[cfg(feature = "write")]
fn insert_animation_track_clone(
    connection: &mut rusqlite::Connection,
    columns: &[String],
    insertion: AnimationTrackCloneInsert<'_>,
) -> Result<()> {
    let column_list = columns
        .iter()
        .map(|column| quote_sql_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let selected = columns
        .iter()
        .map(|column| match column.as_str() {
            "MainId" => "?1".to_owned(),
            "BankId" => "?2".to_owned(),
            "TrackNextIndex" => "0".to_owned(),
            "TrackActionMixer" => "?3".to_owned(),
            "TrackActionMixer2" => "?4".to_owned(),
            "TrackUuid" => "?5".to_owned(),
            "LayerUuidWithTrack" => "?6".to_owned(),
            _ => format!("template.{}", quote_sql_identifier(column)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO Track ({column_list}) SELECT {selected} \
         FROM Track AS template WHERE template.MainId = ?7"
    );

    let transaction = connection.transaction()?;
    let inserted = transaction.execute(
        &sql,
        params![
            insertion.track_id,
            insertion.bank_id,
            insertion.primary_identifier,
            insertion.secondary_identifier,
            insertion.track_uuid.as_slice(),
            insertion.layer_uuid.as_slice(),
            insertion.template_track_id,
        ],
    )?;
    if inserted != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation template {} clone inserted {inserted} rows",
                insertion.template_track_id
            ),
        });
    }
    if insertion.update_elem_scheme {
        let updated = transaction.execute(
            "UPDATE ElemScheme SET MaxIndex = ?1 WHERE TableName = 'Track'",
            params![insertion.track_id],
        )?;
        if updated != 1 {
            return Err(Error::InvalidWrite {
                reason: format!("ElemScheme Track MaxIndex update affected {updated} rows"),
            });
        }
    }
    let changed = if let Some(previous_tail_id) = insertion.previous_tail_id {
        transaction.execute(
            "UPDATE Track SET TrackNextIndex = ?1 \
             WHERE MainId = ?2 AND BankId = ?3 AND (TrackNextIndex IS NULL OR TrackNextIndex = 0)",
            params![insertion.track_id, previous_tail_id, insertion.bank_id],
        )?
    } else {
        transaction.execute(
            "UPDATE TimeLine SET FirstTrack = ?1 \
             WHERE MainId = ?2 AND (FirstTrack IS NULL OR FirstTrack = 0)",
            params![insertion.track_id, insertion.timeline_id],
        )?
    };
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "target timeline {} chain append affected {changed} rows",
                insertion.timeline_id
            ),
        });
    }
    let (_, tail, _) =
        animation_clone_timeline_chain(&transaction, insertion.timeline_id, insertion.limits)?;
    if tail != Some(insertion.track_id) {
        return Err(Error::InvalidWrite {
            reason: format!(
                "target timeline {} did not end at cloned track {}",
                insertion.timeline_id, insertion.track_id
            ),
        });
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(feature = "write")]
fn quote_sql_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(feature = "write")]
fn rollback_animation_additions<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    identifiers: &[Vec<u8>],
) {
    for identifier in identifiers.iter().rev() {
        writer.unstage_new_external_body(identifier);
    }
}

#[cfg(feature = "write")]
struct WritableAnimationTrack {
    bank_id: Option<i64>,
    kind: Option<i64>,
    primary_identifier: Option<Vec<u8>>,
    secondary_identifier: Option<Vec<u8>>,
    value_map: Option<Vec<u8>>,
    primary_size_column: bool,
    secondary_size_column: bool,
    primary_declared_size: Option<i64>,
    secondary_declared_size: Option<i64>,
}

#[cfg(feature = "write")]
fn validate_unique_animation_mixers(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    track_id: i64,
    source: &WritableAnimationTrack,
) -> Result<()> {
    if source.primary_identifier.is_some()
        && source.primary_identifier == source.secondary_identifier
    {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation track {track_id} aliases its primary and secondary mixer identifiers"
            ),
        });
    }
    let secondary_count = if schema.has_column("Track", "TrackActionMixer2") {
        " + (SELECT count(*) FROM Track WHERE CAST(TrackActionMixer2 AS BLOB) = ?1)"
    } else {
        ""
    };
    let sql = format!(
        "SELECT (SELECT count(*) FROM Track \
         WHERE CAST(TrackActionMixer AS BLOB) = ?1){secondary_count}"
    );
    for (label, identifier) in [
        ("primary", source.primary_identifier.as_deref()),
        ("secondary", source.secondary_identifier.as_deref()),
    ] {
        let Some(identifier) = identifier else {
            continue;
        };
        let references: i64 = connection.query_row(&sql, params![identifier], |row| row.get(0))?;
        if references != 1 {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation track {track_id} {label} mixer identifier has {references} Track references, expected exactly one"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
fn writable_animation_track(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    track_id: i64,
) -> Result<WritableAnimationTrack> {
    for column in ["MainId", "TrackActionMixer"] {
        if !schema.has_column("Track", column) {
            return Err(Error::InvalidWrite {
                reason: format!("Track.{column} is required to edit animation curves"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM Track WHERE MainId = ?1",
        params![track_id],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("expected one animation track with ID {track_id}, found {row_count}"),
        });
    }
    let optional = |column: &'static str| {
        if schema.has_column("Track", column) {
            column
        } else {
            "NULL"
        }
    };
    let sql = format!(
        "SELECT {}, {}, TrackActionMixer, {}, {}, {}, {} \
         FROM Track WHERE MainId = ?1 LIMIT 1",
        optional("BankId"),
        optional("TrackKind"),
        optional("TrackActionMixer2"),
        optional("TrackValueMap"),
        optional("TrackActionMixerSize"),
        optional("TrackActionMixer2Size"),
    );
    connection
        .query_row(&sql, params![track_id], |row| {
            Ok(WritableAnimationTrack {
                bank_id: row.get(0)?,
                kind: row.get(1)?,
                primary_identifier: optional_bytes(row.get_ref(2)?, 2, "TrackActionMixer")?
                    .map(<[u8]>::to_vec),
                secondary_identifier: optional_bytes(row.get_ref(3)?, 3, "TrackActionMixer2")?
                    .map(<[u8]>::to_vec),
                value_map: optional_bytes(row.get_ref(4)?, 4, "TrackValueMap")?.map(<[u8]>::to_vec),
                primary_size_column: schema.has_column("Track", "TrackActionMixerSize"),
                secondary_size_column: schema.has_column("Track", "TrackActionMixer2Size"),
                primary_declared_size: row.get(5)?,
                secondary_declared_size: row.get(6)?,
            })
        })
        .optional()?
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation track {track_id} does not exist"),
        })
}

#[cfg(feature = "write")]
fn validate_declared_mixer_size(
    track_id: i64,
    label: &str,
    column_present: bool,
    declared: Option<i64>,
    actual: usize,
) -> Result<()> {
    if !column_present {
        return Ok(());
    }
    let declared = declared.ok_or_else(|| Error::InvalidWrite {
        reason: format!("animation track {track_id} has a NULL {label} mixer size"),
    })?;
    if usize::try_from(declared).ok() != Some(actual) {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation track {track_id} declares {declared} {label} mixer bytes but stores {actual}"
            ),
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
type InstalledAnimationReplacement = (Vec<u8>, Option<Vec<u8>>);

#[cfg(feature = "write")]
fn install_animation_replacements<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    replacements: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<Vec<InstalledAnimationReplacement>> {
    let mut installed = Vec::new();
    for (identifier, body) in replacements {
        match writer.replace_or_update_external_body(&identifier, body) {
            Ok(previous) => installed.push((identifier, previous)),
            Err(error) => {
                rollback_animation_replacements(writer, installed);
                return Err(error);
            }
        }
    }
    Ok(installed)
}

#[cfg(feature = "write")]
fn rollback_animation_replacements<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    installed: Vec<InstalledAnimationReplacement>,
) {
    for (identifier, previous) in installed.into_iter().rev() {
        if let Some(previous) = previous {
            let _ = writer.replace_or_update_external_body(&identifier, previous);
        } else {
            writer.remove_external_replacement(&identifier);
        }
    }
}

#[cfg(feature = "write")]
fn update_animation_track_metadata(
    connection: &rusqlite::Connection,
    track_id: i64,
    primary_size: Option<usize>,
    secondary_size: Option<usize>,
    value_map: Option<&[u8]>,
) -> Result<()> {
    let mut assignments = Vec::new();
    let mut values = Vec::<Value>::new();
    let mut push = |column: &str, value: Value| {
        values.push(value);
        assignments.push(format!("{column} = ?{}", values.len()));
    };
    if let Some(size) = primary_size {
        let size = i64::try_from(size).map_err(|_| Error::OffsetOverflow)?;
        push("TrackActionMixerSize", Value::Integer(size));
    }
    if let Some(size) = secondary_size {
        let size = i64::try_from(size).map_err(|_| Error::OffsetOverflow)?;
        push("TrackActionMixer2Size", Value::Integer(size));
    }
    if let Some(value_map) = value_map {
        push("TrackValueMap", Value::Blob(value_map.to_vec()));
    }
    if assignments.is_empty() {
        return Ok(());
    }
    values.push(Value::Integer(track_id));
    let sql = format!(
        "UPDATE Track SET {} WHERE MainId = ?{}",
        assignments.join(", "),
        values.len()
    );
    let changed = connection.execute(&sql, params_from_iter(values.iter()))?;
    if changed != 1 {
        return Err(Error::InvalidWrite {
            reason: format!("animation track {track_id} is not unique"),
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
fn validate_matching_secondary_curve_keys(
    primary_mixer: &[u8],
    secondary_mixer: &[u8],
    curve_kind: &str,
    axis: Option<&str>,
    limits: Limits,
) -> Result<bool> {
    let primary = parse_animation_curves(primary_mixer, limits)?
        .into_iter()
        .filter(|curve| curve.kind == curve_kind && curve.axis.as_deref() == axis)
        .collect::<Vec<_>>();
    let [primary] = primary.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: format!(
                "primary animation mixer has {} {curve_kind:?} curves for axis {axis:?}, expected one",
                primary.len()
            ),
        });
    };
    let secondary = parse_secondary_animation_curves(secondary_mixer, limits)?
        .into_iter()
        .filter(|curve| curve.kind == curve_kind && curve.axis.as_deref() == axis)
        .collect::<Vec<_>>();
    if secondary.is_empty() {
        return Ok(false);
    }
    if secondary.len() != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "secondary animation mixer has {} {curve_kind:?} curves for axis {axis:?}, expected at most one",
                secondary.len()
            ),
        });
    }
    let secondary = &secondary[0];
    if primary.keyframes.len() != secondary.keyframes.len() {
        return Err(Error::InvalidWrite {
            reason: format!(
                "primary and secondary {curve_kind:?} curves have {} and {} keys",
                primary.keyframes.len(),
                secondary.keyframes.len()
            ),
        });
    }
    for (index, (primary, secondary)) in primary
        .keyframes
        .iter()
        .zip(&secondary.keyframes)
        .enumerate()
    {
        if f64::from(primary.time_60hz) != secondary.time_60hz {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "primary and secondary {curve_kind:?} key {index} times are inconsistent"
                ),
            });
        }
        if primary.tag != secondary.tag {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "primary and secondary {curve_kind:?} key {index} tags are inconsistent"
                ),
            });
        }
    }
    Ok(true)
}

#[cfg(feature = "write")]
fn edit_animation_curve_keyframe<R: Read + Seek>(
    writer: &mut ClipWriter<'_, R>,
    track_id: i64,
    curve_kind: &str,
    axis: Option<&str>,
    edit: CurveKeyEdit<'_>,
    limits: Limits,
) -> Result<AnimationCurveKeyframe> {
    let source = writable_animation_track(
        writer.database().connection(),
        writer.database().schema(),
        track_id,
    )?;
    validate_unique_animation_mixers(
        writer.database().connection(),
        writer.database().schema(),
        track_id,
        &source,
    )?;
    let primary_id = source
        .primary_identifier
        .clone()
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation track {track_id} has no primary action mixer"),
        })?;
    let primary_body =
        writer.external_body_for_update(&primary_id, limits.max_animation_bytes())?;
    let mut primary_mixer = decode_writable_mixer(&primary_body, limits)?;
    validate_declared_mixer_size(
        track_id,
        "primary",
        source.primary_size_column,
        source.primary_declared_size,
        primary_mixer.len(),
    )?;
    let primary_before = primary_mixer.clone();
    let (affected, updated_primary_keys) =
        edit_primary_curve_key(&mut primary_mixer, curve_kind, axis, &edit, limits)?;
    let primary_size = primary_mixer.len();
    let primary_replacement = encode_writable_mixer_for_writer(writer, &primary_mixer, limits)?;

    let secondary_replacement = if let Some(identifier) = source.secondary_identifier.as_deref() {
        let body = writer.external_body_for_update(identifier, limits.max_animation_bytes())?;
        let mut mixer = decode_writable_mixer(&body, limits)?;
        validate_declared_mixer_size(
            track_id,
            "secondary",
            source.secondary_size_column,
            source.secondary_declared_size,
            mixer.len(),
        )?;
        let matching_secondary = validate_matching_secondary_curve_keys(
            &primary_before,
            &mixer,
            curve_kind,
            axis,
            limits,
        )?;
        if source.kind == Some(2000) && curve_kind == "ImageCelName" && !matching_secondary {
            return Err(Error::InvalidWrite {
                reason: "image-cel track has no matching secondary ImageCelName curve".to_owned(),
            });
        }
        let changed = edit_secondary_curve_key(&mut mixer, curve_kind, axis, &edit, limits)?;
        if source.kind == Some(2000) && curve_kind == "ImageCelName" && !changed {
            return Err(Error::InvalidWrite {
                reason: "image-cel track has no matching secondary ImageCelName curve".to_owned(),
            });
        }
        changed
            .then(|| {
                let size = mixer.len();
                encode_writable_mixer_for_writer(writer, &mixer, limits)
                    .map(|body| (identifier.to_vec(), body, size))
            })
            .transpose()?
    } else {
        if source.kind == Some(2000) && curve_kind == "ImageCelName" {
            return Err(Error::InvalidWrite {
                reason: "image-cel track has no secondary action mixer".to_owned(),
            });
        }
        None
    };

    let updated_value_map = if source.kind == Some(2000) && curve_kind == "ImageCelName" {
        if axis.is_some() {
            return Err(Error::InvalidWrite {
                reason: "ImageCelName curve cannot have an axis".to_owned(),
            });
        }
        let map = source
            .value_map
            .as_deref()
            .ok_or_else(|| Error::InvalidWrite {
                reason: "image-cel track has no TrackValueMap".to_owned(),
            })?;
        let bank_id = source.bank_id.ok_or_else(|| Error::InvalidWrite {
            reason: "image-cel Track.BankId is required for current-value synchronization"
                .to_owned(),
        })?;
        let tick = saved_timeline_tick(writer.database().connection(), bank_id)?;
        Some(synchronize_cel_track_value_at_tick(
            map,
            &updated_primary_keys,
            tick,
            limits,
        )?)
    } else {
        None
    };

    let mut replacements = vec![(primary_id, primary_replacement)];
    if let Some((identifier, replacement, _)) = &secondary_replacement {
        replacements.push((identifier.clone(), replacement.clone()));
    }
    let installed = install_animation_replacements(writer, replacements)?;
    let secondary_size = secondary_replacement
        .as_ref()
        .map(|(_, _, size)| *size)
        .or_else(|| {
            source
                .secondary_declared_size
                .and_then(|size| usize::try_from(size).ok())
        });
    if let Err(error) = update_animation_track_metadata(
        writer.database().connection(),
        track_id,
        source.primary_size_column.then_some(primary_size),
        source
            .secondary_size_column
            .then_some(secondary_size)
            .flatten(),
        updated_value_map.as_deref(),
    ) {
        rollback_animation_replacements(writer, installed);
        return Err(error);
    }
    Ok(affected)
}

#[cfg(feature = "write")]
fn saved_timeline_tick(connection: &rusqlite::Connection, bank_id: i64) -> Result<f64> {
    let rows: i64 = connection.query_row(
        "SELECT count(*) FROM TimeLine WHERE BankId = ?1",
        params![bank_id],
        |row| row.get(0),
    )?;
    if rows != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation bank {bank_id} has {rows} timelines; one is required for current-value synchronization"
            ),
        });
    }
    let (frame_rate, current_frame): (f64, Option<f64>) = connection.query_row(
        "SELECT FrameRate, CurrentFrame FROM TimeLine WHERE BankId = ?1 LIMIT 1",
        params![bank_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let current_frame = current_frame.ok_or_else(|| Error::InvalidWrite {
        reason: "animation timeline has no saved current frame".to_owned(),
    })?;
    if !frame_rate.is_finite() || frame_rate <= 0.0 || !current_frame.is_finite() {
        return Err(Error::InvalidWrite {
            reason: "animation timeline has invalid playback metadata".to_owned(),
        });
    }
    Ok(current_frame * 60.0 / frame_rate)
}

#[cfg(feature = "write")]
fn synchronize_cel_track_value_at_tick(
    bytes: &[u8],
    keys: &[AnimationCurveKeyframe],
    tick: f64,
    limits: Limits,
) -> Result<Vec<u8>> {
    let key = keys
        .iter()
        .rev()
        .find(|key| f64::from(key.time_60hz) <= tick + 1e-5)
        .ok_or_else(|| Error::InvalidWrite {
            reason: "edited ImageCelName curve has no key active at the saved frame".to_owned(),
        })?;
    let tag = key.tag.as_ref().ok_or_else(|| Error::InvalidWrite {
        reason: "edited ImageCelName key has no Tag value".to_owned(),
    })?;
    let numeric = exact_cel_numeric_value(key.value)?;
    let mut entries = parse_track_value_map(bytes, limits)?;
    let matching = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.name == "ImageCelName")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = matching.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: "image-cel TrackValueMap must contain exactly one ImageCelName entry"
                .to_owned(),
        });
    };
    let AnimationTrackValue::IndexedText {
        text,
        numeric_value,
    } = &mut entries[*index].value
    else {
        return Err(Error::InvalidWrite {
            reason: "image-cel TrackValueMap entry has an unexpected value kind".to_owned(),
        });
    };
    *text = tag.clone();
    *numeric_value = numeric;
    let encoded = encode_track_value_map(&entries, limits)?;
    if parse_track_value_map(&encoded, limits)? != entries {
        return Err(Error::InvalidWrite {
            reason: "synchronized image-cel TrackValueMap did not round-trip".to_owned(),
        });
    }
    Ok(encoded)
}

#[cfg(feature = "write")]
fn exact_cel_numeric_value(value: f32) -> Result<u32> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f32 {
        return Err(Error::InvalidWrite {
            reason: "ImageCelName numeric values must be exact non-negative integers".to_owned(),
        });
    }
    let numeric = value as u32;
    if numeric as f32 != value {
        return Err(Error::InvalidWrite {
            reason: "ImageCelName numeric value is not exactly representable".to_owned(),
        });
    }
    Ok(numeric)
}

#[cfg(feature = "write")]
fn decode_writable_mixer(body: &[u8], limits: Limits) -> Result<Vec<u8>> {
    let length = body.get(..4).ok_or_else(|| Error::InvalidWrite {
        reason: "animation mixer body lacks its little-endian length prefix".to_owned(),
    })?;
    let declared = u32::from_le_bytes(length.try_into().expect("four-byte slice")) as usize;
    if declared != body.len() - 4 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "animation mixer declares {declared} compressed bytes but stores {}",
                body.len() - 4
            ),
        });
    }
    decompress_mixer(&body[4..], limits.max_animation_bytes())
}

#[cfg(feature = "write")]
fn encode_writable_mixer(mixer: &[u8], limits: Limits) -> Result<Vec<u8>> {
    enforce_byte_limit(
        mixer.len() as u64,
        limits.max_animation_bytes(),
        "animation mixer bytes after replacement",
    )?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(mixer)?;
    let compressed = encoder.finish()?;
    let size = u32::try_from(compressed.len()).map_err(|_| Error::OffsetOverflow)?;
    let mut body = Vec::with_capacity(compressed.len() + 4);
    body.extend_from_slice(&size.to_le_bytes());
    body.extend_from_slice(&compressed);
    enforce_byte_limit(
        body.len() as u64,
        limits.max_animation_bytes(),
        "encoded animation mixer external body",
    )?;
    Ok(body)
}

#[cfg(feature = "write")]
fn encode_writable_mixer_for_writer<R: Read + Seek>(
    writer: &ClipWriter<'_, R>,
    mixer: &[u8],
    limits: Limits,
) -> Result<Vec<u8>> {
    let body = encode_writable_mixer(mixer, limits)?;
    writer.validate_external_body_size_for_update(
        &body,
        limits.max_animation_bytes(),
        "encoded animation mixer external body",
    )?;
    Ok(body)
}

#[cfg(feature = "write")]
#[derive(Clone, Copy)]
struct PrimaryKeyOffsets {
    frame: usize,
    value: usize,
    tag: Option<usize>,
}

#[cfg(feature = "write")]
enum CurveKeyEdit<'a> {
    Insert {
        index: usize,
        key: &'a AnimationCurveKeyframeInsert,
    },
    Remove {
        index: usize,
    },
}

#[cfg(feature = "write")]
impl CurveKeyEdit<'_> {
    const fn index(&self) -> usize {
        match self {
            Self::Insert { index, .. } | Self::Remove { index } => *index,
        }
    }
}

#[cfg(feature = "write")]
struct CurveFieldLayout {
    name: String,
    field_type: String,
    start: usize,
    count_offset: usize,
    data_start: usize,
    data_end: usize,
    end: usize,
    element_size: usize,
}

#[cfg(feature = "write")]
struct CurveRecordLayout {
    start: usize,
    end: usize,
    fields_start: usize,
    key_count: usize,
    fields: Vec<CurveFieldLayout>,
}

#[cfg(feature = "write")]
fn animation_curve_record_layout(
    bytes: &[u8],
    strings: &[String],
    start: usize,
    header: &AnimationCurveHeader,
    key_count: usize,
    secondary: bool,
) -> Result<CurveRecordLayout> {
    let fields_start = header.cursor;
    let mut cursor = fields_start;
    let field_count = read_u32(bytes, &mut cursor)?;
    let mut fields = Vec::new();
    for _ in 0..field_count {
        let start = cursor;
        if secondary {
            let int32_array =
                string_id_optional(strings, "Int32[]").ok_or_else(|| Error::InvalidWrite {
                    reason: "secondary animation mixer lacks Int32[] metadata".to_owned(),
                })?;
            if !secondary_field_header_matches(bytes, cursor, strings.len(), int32_array) {
                return Err(Error::InvalidWrite {
                    reason: "secondary animation curve field metadata is invalid".to_owned(),
                });
            }
            skip_array(bytes, &mut cursor, 3, 4)?;
        }
        let name = string_at(strings, read_u32(bytes, &mut cursor)?)?.to_owned();
        let field_type = string_at(strings, read_u32(bytes, &mut cursor)?)?.to_owned();
        let count_offset = cursor;
        let count =
            usize::try_from(read_u32(bytes, &mut cursor)?).map_err(|_| Error::OffsetOverflow)?;
        if count != key_count {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation curve field {name:?} contains {count} values for {key_count} keys"
                ),
            });
        }
        let element_size =
            animation_curve_element_size(&field_type, secondary).ok_or_else(|| {
                Error::InvalidWrite {
                    reason: format!(
                        "animation curve field {name:?} uses unsupported type {field_type:?}"
                    ),
                }
            })?;
        let data_start = cursor;
        skip_array(bytes, &mut cursor, count, element_size)?;
        let data_end = cursor;
        if [read_u32(bytes, &mut cursor)?, read_u32(bytes, &mut cursor)?] != [0, 0] {
            return Err(Error::InvalidWrite {
                reason: format!("animation curve field {name:?} has a nonzero terminator"),
            });
        }
        fields.push(CurveFieldLayout {
            name,
            field_type,
            start,
            count_offset,
            data_start,
            data_end,
            end: cursor,
            element_size,
        });
    }
    Ok(CurveRecordLayout {
        start,
        end: cursor,
        fields_start,
        key_count,
        fields,
    })
}

#[cfg(feature = "write")]
fn animation_curve_element_size(field_type: &str, secondary: bool) -> Option<usize> {
    match field_type {
        "Byte[]" => Some(1),
        "Single[]" | "String[]" | "Int32[]" => Some(4),
        "UInt32[]" if secondary => Some(4),
        "Float2[]" | "Double[]" => Some(8),
        "Float3[]" => Some(12),
        "Quat[]" | "Double2[]" if secondary => Some(16),
        "Double3[]" if secondary => Some(24),
        "Matrix44[]" => Some(if secondary { 128 } else { 64 }),
        _ => None,
    }
}

#[cfg(feature = "write")]
fn inserted_curve_field_bytes(
    field: &CurveFieldLayout,
    key: &AnimationCurveKeyframeInsert,
    strings: &[String],
    secondary: bool,
) -> Result<Vec<u8>> {
    let finite = |value: Option<f32>, label: &str| -> Result<f32> {
        value
            .filter(|value| value.is_finite())
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!("animation curve insertion requires finite {label}"),
            })
    };
    let string_id = |value: Option<&str>, label: &str| -> Result<Vec<u8>> {
        let value = value.ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation curve insertion requires {label}"),
        })?;
        let id = string_id_optional(strings, value).ok_or_else(|| Error::InvalidWrite {
            reason: format!("animation mixer string table does not contain {value:?}"),
        })?;
        Ok(id.to_le_bytes().to_vec())
    };
    match (field.name.as_str(), field.field_type.as_str(), secondary) {
        ("Frame", "Single[]", _) => Ok(key.time_60hz.to_le_bytes().to_vec()),
        ("Frame", "Double[]", true) => Ok(f64::from(key.time_60hz).to_le_bytes().to_vec()),
        ("Value", "Single[]", _) => Ok(key.value.to_le_bytes().to_vec()),
        ("Value", "Double[]", true) => Ok(f64::from(key.value).to_le_bytes().to_vec()),
        ("Tag", "String[]", _) => string_id(key.tag.as_deref(), "a Tag value"),
        ("Interp", "String[]", _) => string_id(key.interpolation.as_deref(), "an Interp value"),
        ("LeftSlope", "Single[]", _) => {
            Ok(finite(key.left_slope, "LeftSlope")?.to_le_bytes().to_vec())
        }
        ("RightSlope", "Single[]", _) => Ok(finite(key.right_slope, "RightSlope")?
            .to_le_bytes()
            .to_vec()),
        ("LeftSlope", "Double[]", true) => Ok(f64::from(finite(key.left_slope, "LeftSlope")?)
            .to_le_bytes()
            .to_vec()),
        ("RightSlope", "Double[]", true) => Ok(f64::from(finite(key.right_slope, "RightSlope")?)
            .to_le_bytes()
            .to_vec()),
        ("ReviseConstant", "Byte[]", _) => Ok(vec![key.revise_constant.ok_or_else(|| {
            Error::InvalidWrite {
                reason: "animation curve insertion requires a ReviseConstant value".to_owned(),
            }
        })?]),
        _ => Err(Error::InvalidWrite {
            reason: format!(
                "cannot synthesize animation curve field {:?} with type {:?}",
                field.name, field.field_type
            ),
        }),
    }
}

#[cfg(feature = "write")]
fn rebuild_curve_record(
    bytes: &mut Vec<u8>,
    layout: &CurveRecordLayout,
    edit: &CurveKeyEdit<'_>,
    strings: &[String],
    secondary: bool,
) -> Result<()> {
    let index = edit.index();
    let new_count = match edit {
        CurveKeyEdit::Insert { .. } => {
            if index > layout.key_count {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "animation curve has {} keys, so insertion index {index} is invalid",
                        layout.key_count
                    ),
                });
            }
            layout
                .key_count
                .checked_add(1)
                .ok_or(Error::OffsetOverflow)?
        }
        CurveKeyEdit::Remove { .. } => {
            if index >= layout.key_count {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "animation curve has {} keys, so removal index {index} is invalid",
                        layout.key_count
                    ),
                });
            }
            if layout.key_count == 1 {
                return Err(Error::InvalidWrite {
                    reason: "removing the final animation curve key is not supported".to_owned(),
                });
            }
            layout.key_count - 1
        }
    };
    let encoded_count = u32::try_from(new_count).map_err(|_| Error::OffsetOverflow)?;
    let mut record = Vec::new();
    record.extend_from_slice(&bytes[layout.start..layout.fields_start + 4]);
    for field in &layout.fields {
        record.extend_from_slice(&bytes[field.start..field.count_offset]);
        record.extend_from_slice(&encoded_count.to_le_bytes());
        let split = field
            .data_start
            .checked_add(
                index
                    .checked_mul(field.element_size)
                    .ok_or(Error::OffsetOverflow)?,
            )
            .ok_or(Error::OffsetOverflow)?;
        record.extend_from_slice(&bytes[field.data_start..split]);
        if let CurveKeyEdit::Insert { key, .. } = edit {
            let inserted = inserted_curve_field_bytes(field, key, strings, secondary)?;
            if inserted.len() != field.element_size {
                return Err(Error::InvalidWrite {
                    reason: format!(
                        "animation curve field {:?} encoded {} bytes instead of {}",
                        field.name,
                        inserted.len(),
                        field.element_size
                    ),
                });
            }
            record.extend_from_slice(&inserted);
            record.extend_from_slice(&bytes[split..field.data_end]);
        } else {
            let after = split
                .checked_add(field.element_size)
                .ok_or(Error::OffsetOverflow)?;
            record.extend_from_slice(&bytes[after..field.data_end]);
        }
        record.extend_from_slice(&bytes[field.data_end..field.end]);
    }
    bytes.splice(layout.start..layout.end, record);
    Ok(())
}

#[cfg(feature = "write")]
fn prepare_curve_insert_strings(
    bytes: &mut Vec<u8>,
    insertion: &AnimationCurveKeyframeInsert,
    limits: Limits,
) -> Result<()> {
    for value in [insertion.tag.as_deref(), insertion.interpolation.as_deref()]
        .into_iter()
        .flatten()
    {
        ensure_mixer_string(bytes, value, limits)?;
    }
    Ok(())
}

#[cfg(feature = "write")]
fn edit_primary_curve_key(
    bytes: &mut Vec<u8>,
    curve_kind: &str,
    axis: Option<&str>,
    edit: &CurveKeyEdit<'_>,
    limits: Limits,
) -> Result<(AnimationCurveKeyframe, Vec<AnimationCurveKeyframe>)> {
    if let CurveKeyEdit::Insert { key, .. } = edit {
        if !key.time_60hz.is_finite() || !key.value.is_finite() {
            return Err(Error::InvalidWrite {
                reason: "animation curve insertion must use finite numeric values".to_owned(),
            });
        }
        prepare_curve_insert_strings(bytes, key, limits)?;
    }
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let fcurve = string_id_optional(&strings, "FCurve").ok_or_else(|| Error::InvalidWrite {
        reason: "animation mixer has no FCurve string".to_owned(),
    })?;
    let mut found = None;
    for start in data_start..=bytes.len().saturating_sub(12) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        if header.kind != curve_kind || header.axis.as_deref() != axis {
            continue;
        }
        if found.is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation mixer has multiple {curve_kind:?} curves for axis {axis:?}"
                ),
            });
        }
        let mut cursor = header.cursor;
        let curve = parse_animation_curve_fields(
            bytes,
            &strings,
            &mut cursor,
            header.kind.clone(),
            header.axis.clone(),
            limits,
        )?;
        let removed = match edit {
            CurveKeyEdit::Insert { .. } => None,
            CurveKeyEdit::Remove { index } => Some(
                curve.keyframes.get(*index).cloned().ok_or_else(|| {
                    Error::InvalidWrite {
                        reason: format!(
                            "animation curve {curve_kind:?} has {} keys, so index {index} is invalid",
                            curve.keyframes.len()
                        ),
                    }
                })?,
            ),
        };
        let layout = animation_curve_record_layout(
            bytes,
            &strings,
            start,
            &header,
            curve.keyframes.len(),
            false,
        )?;
        found = Some((removed, layout));
    }
    let (removed, layout) = found.ok_or_else(|| Error::InvalidWrite {
        reason: format!("animation mixer has no {curve_kind:?} curve for axis {axis:?}"),
    })?;
    rebuild_curve_record(bytes, &layout, edit, &strings, false)?;
    let curves = parse_animation_curves(bytes, limits)?;
    let matching = curves
        .into_iter()
        .filter(|curve| curve.kind == curve_kind && curve.axis.as_deref() == axis)
        .collect::<Vec<_>>();
    let [curve] = matching.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: "edited primary animation curve did not round-trip uniquely".to_owned(),
        });
    };
    let affected = match edit {
        CurveKeyEdit::Insert { index, .. } => {
            curve
                .keyframes
                .get(*index)
                .cloned()
                .ok_or_else(|| Error::InvalidWrite {
                    reason: "inserted animation curve key did not round-trip".to_owned(),
                })?
        }
        CurveKeyEdit::Remove { .. } => removed.expect("remove edit captured its original key"),
    };
    Ok((affected, curve.keyframes.clone()))
}

#[cfg(feature = "write")]
fn edit_secondary_curve_key(
    bytes: &mut Vec<u8>,
    curve_kind: &str,
    axis: Option<&str>,
    edit: &CurveKeyEdit<'_>,
    limits: Limits,
) -> Result<bool> {
    if let CurveKeyEdit::Insert { key, .. } = edit {
        prepare_curve_insert_strings(bytes, key, limits)?;
    }
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let Some(fcurve) = string_id_optional(&strings, "FCurve") else {
        return Ok(false);
    };
    let Some(int32_array) = string_id_optional(&strings, "Int32[]") else {
        return Ok(false);
    };
    let mut found = None;
    for start in data_start..=bytes.len().saturating_sub(12) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        if header.kind != curve_kind || header.axis.as_deref() != axis {
            continue;
        }
        let mut cursor = header.cursor;
        let field_count = read_u32(bytes, &mut cursor)?;
        if !secondary_field_header_matches(bytes, cursor, strings.len(), int32_array) {
            continue;
        }
        if found.is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "secondary animation mixer has multiple {curve_kind:?} curves for axis {axis:?}"
                ),
            });
        }
        let curve = parse_secondary_animation_curve_fields(
            bytes,
            &strings,
            &mut cursor,
            header.kind.clone(),
            header.axis.clone(),
            field_count,
            limits,
        )?;
        let layout = animation_curve_record_layout(
            bytes,
            &strings,
            start,
            &header,
            curve.keyframes.len(),
            true,
        )?;
        found = Some(layout);
    }
    let Some(layout) = found else {
        return Ok(false);
    };
    rebuild_curve_record(bytes, &layout, edit, &strings, true)?;
    let matching = parse_secondary_animation_curves(bytes, limits)?
        .into_iter()
        .filter(|curve| curve.kind == curve_kind && curve.axis.as_deref() == axis)
        .count();
    if matching != 1 {
        return Err(Error::InvalidWrite {
            reason: "edited secondary animation curve did not round-trip uniquely".to_owned(),
        });
    }
    Ok(true)
}

#[cfg(feature = "write")]
fn primary_key_offsets(
    bytes: &[u8],
    curve_kind: &str,
    axis: Option<&str>,
    key_index: usize,
    limits: Limits,
) -> Result<(AnimationCurveKeyframe, PrimaryKeyOffsets)> {
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let fcurve = string_id_optional(&strings, "FCurve").ok_or_else(|| Error::InvalidWrite {
        reason: "animation mixer has no FCurve string".to_owned(),
    })?;
    let mut found = None;
    for start in data_start..=bytes.len().saturating_sub(12) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        if header.kind != curve_kind || header.axis.as_deref() != axis {
            continue;
        }
        if found.is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "animation mixer has multiple {curve_kind:?} curves for axis {axis:?}"
                ),
            });
        }
        let mut validated_cursor = header.cursor;
        let curve = parse_animation_curve_fields(
            bytes,
            &strings,
            &mut validated_cursor,
            header.kind.clone(),
            header.axis.clone(),
            limits,
        )?;
        let key = curve
            .keyframes
            .get(key_index)
            .cloned()
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!(
                    "animation curve {curve_kind:?} has {} keys, so index {key_index} is invalid",
                    curve.keyframes.len()
                ),
            })?;
        let offsets = locate_primary_key_fields(bytes, &strings, header.cursor, key_index)?;
        found = Some((key, offsets));
    }
    found.ok_or_else(|| Error::InvalidWrite {
        reason: format!("animation mixer has no {curve_kind:?} curve for axis {axis:?}"),
    })
}

#[cfg(feature = "write")]
fn locate_primary_key_fields(
    bytes: &[u8],
    strings: &[String],
    mut cursor: usize,
    key_index: usize,
) -> Result<PrimaryKeyOffsets> {
    let field_count = read_u32(bytes, &mut cursor)?;
    let mut frame = None;
    let mut value = None;
    let mut tag = None;
    for _ in 0..field_count {
        let field = string_at(strings, read_u32(bytes, &mut cursor)?)?;
        let field_type = string_at(strings, read_u32(bytes, &mut cursor)?)?;
        let count = read_u32(bytes, &mut cursor)? as usize;
        let data = cursor;
        match field_type {
            "Single[]" | "String[]" | "Int32[]" => {
                if key_index < count {
                    let offset = data
                        .checked_add(key_index.checked_mul(4).ok_or(Error::OffsetOverflow)?)
                        .ok_or(Error::OffsetOverflow)?;
                    match (field, field_type) {
                        ("Frame", "Single[]") => frame = Some(offset),
                        ("Value", "Single[]") => value = Some(offset),
                        ("Tag", "String[]") => tag = Some(offset),
                        _ => {}
                    }
                }
                skip_array(bytes, &mut cursor, count, 4)?;
            }
            "Byte[]" => skip(bytes, &mut cursor, count)?,
            "Float2[]" => skip_array(bytes, &mut cursor, count, 8)?,
            "Float3[]" => skip_array(bytes, &mut cursor, count, 12)?,
            "Quat[]" => skip_array(bytes, &mut cursor, count, 16)?,
            "Matrix44[]" => skip_array(bytes, &mut cursor, count, 64)?,
            other => {
                return Err(animation_error(format!(
                    "unsupported FCurve field type {other:?} while locating a write"
                )));
            }
        }
        if [read_u32(bytes, &mut cursor)?, read_u32(bytes, &mut cursor)?] != [0, 0] {
            return Err(animation_error(
                "FCurve field has a nonzero terminator while locating a write",
            ));
        }
    }
    Ok(PrimaryKeyOffsets {
        frame: frame.ok_or_else(|| Error::InvalidWrite {
            reason: "animation curve key has no writable Frame value".to_owned(),
        })?,
        value: value.ok_or_else(|| Error::InvalidWrite {
            reason: "animation curve key has no writable Value value".to_owned(),
        })?,
        tag,
    })
}

#[cfg(feature = "write")]
fn patch_primary_curve_numeric(
    bytes: &mut [u8],
    curve_kind: &str,
    axis: Option<&str>,
    key_index: usize,
    time_60hz: f32,
    value: f32,
    limits: Limits,
) -> Result<AnimationCurveKeyframe> {
    let (original, offsets) = primary_key_offsets(bytes, curve_kind, axis, key_index, limits)?;
    bytes[offsets.frame..offsets.frame + 4].copy_from_slice(&time_60hz.to_le_bytes());
    bytes[offsets.value..offsets.value + 4].copy_from_slice(&value.to_le_bytes());
    let (updated, _) = primary_key_offsets(bytes, curve_kind, axis, key_index, limits)?;
    if updated.time_60hz != time_60hz || updated.value != value {
        return Err(Error::InvalidWrite {
            reason: "primary animation curve replacement did not round-trip".to_owned(),
        });
    }
    Ok(original)
}

#[cfg(feature = "write")]
fn ensure_mixer_string(bytes: &mut Vec<u8>, value: &str, limits: Limits) -> Result<u32> {
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    if let Some(id) = string_id_optional(&strings, value) {
        return Ok(id);
    }
    if value.len() > u8::MAX as usize {
        return Err(Error::InvalidWrite {
            reason: "animation mixer strings cannot exceed 255 UTF-8 bytes".to_owned(),
        });
    }
    let count = u32::try_from(strings.len()).map_err(|_| Error::OffsetOverflow)?;
    enforce_item_limit(
        u64::from(count) + 1,
        limits.max_animation_items(),
        "animation mixer strings after replacement",
    )?;
    let mut encoded = Vec::with_capacity(value.len() + 1);
    encoded.push(value.len() as u8);
    encoded.extend_from_slice(value.as_bytes());
    bytes.splice(data_start..data_start, encoded);
    bytes[16..20].copy_from_slice(&(count + 1).to_le_bytes());
    Ok(count)
}

#[cfg(feature = "write")]
fn patch_primary_curve_tag(
    bytes: &mut Vec<u8>,
    key_index: usize,
    tag: &str,
    limits: Limits,
) -> Result<String> {
    let id = ensure_mixer_string(bytes, tag, limits)?;
    let (original, offsets) = primary_key_offsets(bytes, "ImageCelName", None, key_index, limits)?;
    let tag_offset = offsets.tag.ok_or_else(|| Error::InvalidWrite {
        reason: "ImageCelName curve has no writable Tag array".to_owned(),
    })?;
    bytes[tag_offset..tag_offset + 4].copy_from_slice(&id.to_le_bytes());
    let (updated, _) = primary_key_offsets(bytes, "ImageCelName", None, key_index, limits)?;
    if updated.tag.as_deref() != Some(tag) {
        return Err(Error::InvalidWrite {
            reason: "cel tag replacement did not round-trip".to_owned(),
        });
    }
    original.tag.ok_or_else(|| Error::InvalidWrite {
        reason: "ImageCelName key has no original tag".to_owned(),
    })
}

#[cfg(feature = "write")]
#[derive(Clone, Copy)]
struct SecondaryKeyOffsets {
    frame: usize,
    value: usize,
    tag: Option<usize>,
}

#[cfg(feature = "write")]
fn secondary_key_offsets(
    bytes: &[u8],
    curve_kind: &str,
    axis: Option<&str>,
    key_index: usize,
    limits: Limits,
) -> Result<Option<(SecondaryAnimationCurveKeyframe, SecondaryKeyOffsets)>> {
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let Some(fcurve) = string_id_optional(&strings, "FCurve") else {
        return Ok(None);
    };
    let Some(int32_array) = string_id_optional(&strings, "Int32[]") else {
        return Ok(None);
    };
    let mut found = None;
    for start in data_start..=bytes.len().saturating_sub(12) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        if header.kind != curve_kind || header.axis.as_deref() != axis {
            continue;
        }
        let mut validated_cursor = header.cursor;
        let field_count = read_u32(bytes, &mut validated_cursor)?;
        if !secondary_field_header_matches(bytes, validated_cursor, strings.len(), int32_array) {
            continue;
        }
        if found.is_some() {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "secondary animation mixer has multiple {curve_kind:?} curves for axis {axis:?}"
                ),
            });
        }
        let curve = parse_secondary_animation_curve_fields(
            bytes,
            &strings,
            &mut validated_cursor,
            header.kind,
            header.axis,
            field_count,
            limits,
        )?;
        let key = curve
            .keyframes
            .get(key_index)
            .cloned()
            .ok_or_else(|| Error::InvalidWrite {
                reason: format!(
                    "secondary animation curve {curve_kind:?} has {} keys, so index {key_index} is invalid",
                    curve.keyframes.len()
                ),
            })?;
        let offsets = locate_secondary_key_fields(bytes, &strings, header.cursor, key_index)?;
        found = Some((key, offsets));
    }
    Ok(found)
}

#[cfg(feature = "write")]
fn locate_secondary_key_fields(
    bytes: &[u8],
    strings: &[String],
    mut cursor: usize,
    key_index: usize,
) -> Result<SecondaryKeyOffsets> {
    let field_count = read_u32(bytes, &mut cursor)?;
    let mut frame = None;
    let mut value = None;
    let mut tag = None;
    for _ in 0..field_count {
        skip_array(bytes, &mut cursor, 3, 4)?;
        let field = string_at(strings, read_u32(bytes, &mut cursor)?)?;
        let field_type = string_at(strings, read_u32(bytes, &mut cursor)?)?;
        let count = read_u32(bytes, &mut cursor)? as usize;
        let data = cursor;
        match field_type {
            "Double[]" => {
                if key_index < count {
                    let offset = data
                        .checked_add(key_index.checked_mul(8).ok_or(Error::OffsetOverflow)?)
                        .ok_or(Error::OffsetOverflow)?;
                    match field {
                        "Frame" => frame = Some(offset),
                        "Value" => value = Some(offset),
                        _ => {}
                    }
                }
                skip_array(bytes, &mut cursor, count, 8)?;
            }
            "Single[]" | "String[]" | "Int32[]" | "UInt32[]" => {
                if field_type == "String[]" && field == "Tag" && key_index < count {
                    tag = Some(
                        data.checked_add(key_index.checked_mul(4).ok_or(Error::OffsetOverflow)?)
                            .ok_or(Error::OffsetOverflow)?,
                    );
                }
                skip_array(bytes, &mut cursor, count, 4)?;
            }
            "Byte[]" => skip(bytes, &mut cursor, count)?,
            "Float2[]" => skip_array(bytes, &mut cursor, count, 8)?,
            "Float3[]" => skip_array(bytes, &mut cursor, count, 12)?,
            "Double2[]" => skip_array(bytes, &mut cursor, count, 16)?,
            "Double3[]" => skip_array(bytes, &mut cursor, count, 24)?,
            "Quat[]" => skip_array(bytes, &mut cursor, count, 32)?,
            "Matrix44[]" => skip_array(bytes, &mut cursor, count, 128)?,
            other => {
                return Err(animation_error(format!(
                    "unsupported secondary FCurve field type {other:?} while locating a write"
                )));
            }
        }
        if [read_u32(bytes, &mut cursor)?, read_u32(bytes, &mut cursor)?] != [0, 0] {
            return Err(animation_error(
                "secondary FCurve field has a nonzero terminator while locating a write",
            ));
        }
    }
    Ok(SecondaryKeyOffsets {
        frame: frame.ok_or_else(|| Error::InvalidWrite {
            reason: "secondary animation curve key has no writable Frame value".to_owned(),
        })?,
        value: value.ok_or_else(|| Error::InvalidWrite {
            reason: "secondary animation curve key has no writable Value value".to_owned(),
        })?,
        tag,
    })
}

#[cfg(feature = "write")]
fn patch_secondary_curve_numeric(
    bytes: &mut [u8],
    curve_kind: &str,
    axis: Option<&str>,
    key_index: usize,
    time_60hz: f64,
    value: f64,
    limits: Limits,
) -> Result<()> {
    let Some((_, offsets)) = secondary_key_offsets(bytes, curve_kind, axis, key_index, limits)?
    else {
        return Ok(());
    };
    bytes[offsets.frame..offsets.frame + 8].copy_from_slice(&time_60hz.to_le_bytes());
    bytes[offsets.value..offsets.value + 8].copy_from_slice(&value.to_le_bytes());
    let Some((updated, _)) = secondary_key_offsets(bytes, curve_kind, axis, key_index, limits)?
    else {
        return Err(Error::InvalidWrite {
            reason: "secondary animation curve disappeared after replacement".to_owned(),
        });
    };
    if updated.time_60hz != time_60hz || updated.value != value {
        return Err(Error::InvalidWrite {
            reason: "secondary animation curve replacement did not round-trip".to_owned(),
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
fn patch_secondary_curve_tag(
    bytes: &mut Vec<u8>,
    key_index: usize,
    tag: &str,
    limits: Limits,
) -> Result<Option<String>> {
    if secondary_key_offsets(bytes, "ImageCelName", None, key_index, limits)?.is_none() {
        return Ok(None);
    }
    let id = ensure_mixer_string(bytes, tag, limits)?;
    let Some((original, offsets)) =
        secondary_key_offsets(bytes, "ImageCelName", None, key_index, limits)?
    else {
        return Err(Error::InvalidWrite {
            reason: "secondary ImageCelName curve disappeared after adding its tag string"
                .to_owned(),
        });
    };
    let tag_offset = offsets.tag.ok_or_else(|| Error::InvalidWrite {
        reason: "secondary ImageCelName curve has no writable Tag array".to_owned(),
    })?;
    bytes[tag_offset..tag_offset + 4].copy_from_slice(&id.to_le_bytes());
    let Some((updated, _)) = secondary_key_offsets(bytes, "ImageCelName", None, key_index, limits)?
    else {
        return Err(Error::InvalidWrite {
            reason: "secondary ImageCelName curve disappeared after replacement".to_owned(),
        });
    };
    if updated.tag.as_deref() != Some(tag) {
        return Err(Error::InvalidWrite {
            reason: "secondary cel tag replacement did not round-trip".to_owned(),
        });
    }
    Ok(Some(original.tag.ok_or_else(|| Error::InvalidWrite {
        reason: "secondary ImageCelName key has no original tag".to_owned(),
    })?))
}

#[cfg(feature = "write")]
fn synchronize_cel_track_value(
    bytes: &[u8],
    original_tag: &str,
    replacement_tag: &str,
    limits: Limits,
) -> Result<Option<Vec<u8>>> {
    let mut entries = parse_track_value_map(bytes, limits)?;
    let matches = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.name == "ImageCelName")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(None);
    }
    let [index] = matches.as_slice() else {
        return Err(Error::InvalidWrite {
            reason: "animation track repeats ImageCelName in TrackValueMap".to_owned(),
        });
    };
    let AnimationTrackValue::IndexedText { text, .. } = &mut entries[*index].value else {
        return Err(Error::InvalidWrite {
            reason: "animation track ImageCelName current value is not typed text".to_owned(),
        });
    };
    if text != original_tag {
        return Ok(None);
    }
    *text = replacement_tag.to_owned();
    let encoded = encode_track_value_map(&entries, limits)?;
    Ok((encoded != bytes).then_some(encoded))
}

struct AnimationTrackSource {
    id: i64,
    kind: i64,
    next_track_id: Option<i64>,
    external_identifier: Option<Box<[u8]>>,
    secondary_external_identifier: Option<Box<[u8]>>,
    value_map: Option<Box<[u8]>>,
    layer_uuid: Option<[u8; 16]>,
}

fn preferred_timeline_id(database: &Database) -> Result<Option<i64>> {
    if !database
        .schema()
        .has_column("AnimationCutBank", "FirstTimeLine")
    {
        return Ok(None);
    }
    let sql = if database.schema().has_column("AnimationCutBank", "Enable") {
        "SELECT FirstTimeLine FROM AnimationCutBank \
         WHERE Enable != 0 ORDER BY MainId LIMIT 1"
    } else {
        "SELECT FirstTimeLine FROM AnimationCutBank ORDER BY MainId LIMIT 1"
    };
    Ok(database
        .connection()
        .query_row(sql, [], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten()
        .filter(|id| *id != 0))
}

fn animation_track_sources(
    database: &Database,
    timeline: &Timeline,
    limits: Limits,
) -> Result<Vec<AnimationTrackSource>> {
    if database.schema().table("Track").is_none() {
        if timeline.first_track_id.is_none() {
            return Ok(Vec::new());
        }
        return Err(animation_error("timeline references a missing Track table"));
    }
    for column in [
        "MainId",
        "BankId",
        "TrackKind",
        "TrackActionMixer",
        "LayerUuidWithTrack",
    ] {
        database.require_column("Track", column)?;
    }
    let optional = |column: &'static str| {
        if database.schema().has_column("Track", column) {
            column
        } else {
            "NULL"
        }
    };
    let sql = format!(
        "SELECT MainId, TrackKind, TrackActionMixer, LayerUuidWithTrack, {}, {}, {} FROM Track \
         WHERE BankId = ?1 ORDER BY MainId",
        optional("TrackActionMixer2"),
        optional("TrackValueMap"),
        optional("TrackNextIndex"),
    );
    let mut statement = database.connection().prepare(&sql)?;
    let mut rows = statement.query(params![timeline.bank_id])?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        enforce_item_limit(
            sources.len() as u64 + 1,
            limits.max_animation_items(),
            "animation tracks",
        )?;
        let external = optional_bytes(row.get_ref(2)?, 2, "TrackActionMixer")?;
        if let Some(value) = external {
            enforce_byte_limit(
                value.len() as u64,
                limits.max_identifier_size(),
                "animation external identifier",
            )?;
        }
        let secondary = optional_bytes(row.get_ref(4)?, 4, "TrackActionMixer2")?;
        if let Some(value) = secondary {
            enforce_byte_limit(
                value.len() as u64,
                limits.max_identifier_size(),
                "secondary animation external identifier",
            )?;
        }
        let value_map = optional_bytes(row.get_ref(5)?, 5, "TrackValueMap")?;
        if let Some(value) = value_map {
            enforce_byte_limit(
                value.len() as u64,
                limits.max_animation_bytes(),
                "animation track value map",
            )?;
        }
        let uuid = optional_bytes(row.get_ref(3)?, 3, "LayerUuidWithTrack")?;
        if let Some(value) = uuid {
            enforce_byte_limit(
                value.len() as u64,
                limits.max_identifier_size(),
                "animation layer UUID",
            )?;
        }
        sources.push(AnimationTrackSource {
            id: row.get(0)?,
            kind: row.get(1)?,
            next_track_id: nonzero_track_id(row.get(6)?),
            external_identifier: external.map(Box::from),
            secondary_external_identifier: secondary.map(Box::from),
            value_map: value_map.map(Box::from),
            layer_uuid: uuid.map(normalize_uuid).transpose()?,
        });
    }
    let has_track_chain = database.schema().has_column("Track", "TrackNextIndex");
    if let Some(first_track_id) = timeline.first_track_id.filter(|_| has_track_chain) {
        validate_track_chain(&sources, first_track_id)?;
    }
    Ok(sources)
}

fn validate_track_chain(sources: &[AnimationTrackSource], first_track_id: i64) -> Result<()> {
    let by_id = sources
        .iter()
        .map(|source| (source.id, source.next_track_id))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != sources.len() {
        return Err(animation_error("timeline contains duplicate track IDs"));
    }
    let mut current = Some(first_track_id);
    let mut visited = BTreeMap::new();
    while let Some(id) = current {
        if visited.insert(id, ()).is_some() {
            return Err(animation_error(format!(
                "timeline track chain is cyclic at track {id}"
            )));
        }
        current = *by_id
            .get(&id)
            .ok_or_else(|| animation_error(format!("timeline track {id} is missing")))?;
    }
    if visited.len() != sources.len() {
        return Err(animation_error(
            "timeline track chain contains unreachable tracks",
        ));
    }
    Ok(())
}

fn nonzero_track_id(value: Option<i64>) -> Option<i64> {
    value.filter(|value| *value != 0)
}

fn layer_uuid_ids(database: &Database, limits: Limits) -> Result<BTreeMap<[u8; 16], i64>> {
    for column in ["MainId", "LayerUuid"] {
        database.require_column("Layer", column)?;
    }
    let mut statement = database.connection().prepare(
        "SELECT MainId, LayerUuid FROM Layer WHERE LayerUuid IS NOT NULL ORDER BY MainId",
    )?;
    let mut rows = statement.query([])?;
    let mut layers = BTreeMap::new();
    while let Some(row) = rows.next()? {
        enforce_item_limit(
            layers.len() as u64 + 1,
            limits.max_animation_items(),
            "animation layer UUIDs",
        )?;
        let raw = required_bytes(row.get_ref(1)?, 1, "LayerUuid")?;
        enforce_byte_limit(
            raw.len() as u64,
            limits.max_identifier_size(),
            "animation layer UUID",
        )?;
        let uuid = normalize_uuid(raw)?;
        let id: i64 = row.get(0)?;
        if let Some(previous) = layers.insert(uuid, id) {
            return Err(animation_error(format!(
                "layers {previous} and {id} have the same normalized UUID"
            )));
        }
    }
    Ok(layers)
}

fn normalize_uuid(bytes: &[u8]) -> Result<[u8; 16]> {
    if let Ok(uuid) = <[u8; 16]>::try_from(bytes) {
        return Ok(uuid);
    }
    let text = str::from_utf8(bytes)
        .map_err(|_| animation_error("layer UUID is neither 16 raw bytes nor UTF-8 text"))?;
    let hex = text
        .bytes()
        .filter(u8::is_ascii_hexdigit)
        .collect::<Vec<_>>();
    if hex.len() != 32 {
        return Err(animation_error(format!(
            "text layer UUID has {} hexadecimal digits instead of 32",
            hex.len()
        )));
    }
    let mut uuid = [0_u8; 16];
    for (index, pair) in hex.chunks_exact(2).enumerate() {
        uuid[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Ok(uuid)
}

fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(animation_error("UUID contains a non-hexadecimal digit")),
    }
}

fn decompress_mixer(compressed: &[u8], limit: u64) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut data = Vec::new();
    decoder
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(Error::LimitExceeded {
            resource: "decompressed animation mixer bytes",
            value: data.len() as u64,
            limit,
        });
    }
    Ok(data)
}

#[cfg(test)]
fn parse_image_cel_curve(bytes: &[u8], limits: Limits) -> Result<Option<Vec<CelKeyframe>>> {
    let Some(curve) = parse_animation_curves(bytes, limits)?
        .into_iter()
        .find(|curve| curve.kind == "ImageCelName")
    else {
        return Ok(None);
    };
    curve
        .keyframes
        .into_iter()
        .map(|key| {
            Ok(CelKeyframe {
                time_60hz: key.time_60hz,
                value: key.value,
                tag: key
                    .tag
                    .ok_or_else(|| animation_error("ImageCelName key has no Tag value"))?,
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn parse_camera_2d_track_values(
    values: &[AnimationTrackValueEntry],
) -> Result<Camera2DTrackValues> {
    let vector = |name| -> Result<Camera2DPoint> {
        let value = unique_track_value(values, name)?;
        let AnimationTrackValue::Vector2 { x, y } = value else {
            return Err(animation_error(format!(
                "2D-camera value {name:?} is not a two-dimensional value"
            )));
        };
        Ok(Camera2DPoint { x: *x, y: *y })
    };
    let scalar = |name| -> Result<f64> {
        let value = unique_track_value(values, name)?;
        let AnimationTrackValue::Float(value) = value else {
            return Err(animation_error(format!(
                "2D-camera value {name:?} is not scalar"
            )));
        };
        Ok(*value)
    };
    Ok(Camera2DTrackValues {
        image_center: vector("ImageCenter")?,
        image_position: vector("ImagePosition")?,
        rotation: scalar("ImageRotation")?,
        scale: scalar("ImageScale")?,
        opacity: scalar("Opacity")?,
    })
}

fn unique_track_value<'a>(
    values: &'a [AnimationTrackValueEntry],
    name: &str,
) -> Result<&'a AnimationTrackValue> {
    let mut matches = values.iter().filter(|entry| entry.name == name);
    let value = matches
        .next()
        .ok_or_else(|| animation_error(format!("2D-camera track lacks {name:?}")))?;
    if matches.next().is_some() {
        return Err(animation_error(format!("2D-camera track repeats {name:?}")));
    }
    Ok(&value.value)
}

fn parse_track_value_map(bytes: &[u8], limits: Limits) -> Result<Vec<AnimationTrackValueEntry>> {
    enforce_byte_limit(
        bytes.len() as u64,
        limits.max_animation_bytes(),
        "animation track value map",
    )?;
    let mut cursor = 0;
    let header_size = read_be_u32(bytes, &mut cursor)?;
    if header_size != 8 {
        return Err(animation_error(format!(
            "TrackValueMap header size is {header_size} instead of 8"
        )));
    }
    let count = read_be_u32(bytes, &mut cursor)?;
    enforce_item_limit(
        u64::from(count),
        limits.max_animation_items(),
        "animation track values",
    )?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count as usize)
        .map_err(|_| Error::LimitExceeded {
            resource: "animation track value allocation",
            value: u64::from(count),
            limit: limits.max_animation_items(),
        })?;
    for _ in 0..count {
        let record_start = cursor;
        let record_size = read_be_u32(bytes, &mut cursor)? as usize;
        let record_end = record_start
            .checked_add(record_size)
            .ok_or(Error::OffsetOverflow)?;
        if record_end > bytes.len() {
            return Err(animation_error(
                "TrackValueMap record exceeds the inline value map",
            ));
        }
        let name = read_utf16be_value(bytes, &mut cursor, record_end)?;
        let text = read_utf16be_value(bytes, &mut cursor, record_end)?;
        let kind = read_be_u32_bounded(bytes, &mut cursor, record_end)?;
        let payload = bytes
            .get(cursor..record_end)
            .ok_or_else(|| animation_error("TrackValueMap record fields exceed its size"))?;
        let value = match (kind, payload) {
            (0, [a, b, c, d, e, f, g, h]) if text.is_empty() => {
                let value = f64::from_bits(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h]));
                if !value.is_finite() {
                    return Err(animation_error(
                        "TrackValueMap contains a non-finite floating-point value",
                    ));
                }
                AnimationTrackValue::Float(value)
            }
            (2, [a, b, c, d]) => AnimationTrackValue::IndexedText {
                text,
                numeric_value: u32::from_be_bytes([*a, *b, *c, *d]),
            },
            (3, payload) if text.is_empty() && payload.len() == 16 => {
                let x = f64::from_be_bytes(payload[..8].try_into().expect("eight bytes"));
                let y = f64::from_be_bytes(payload[8..].try_into().expect("eight bytes"));
                if !x.is_finite() || !y.is_finite() {
                    return Err(animation_error(
                        "TrackValueMap contains a non-finite two-dimensional value",
                    ));
                }
                AnimationTrackValue::Vector2 { x, y }
            }
            _ => AnimationTrackValue::Unknown {
                kind,
                text,
                payload: Box::from(payload),
            },
        };
        cursor = record_end;
        entries.push(AnimationTrackValueEntry { name, value });
    }
    if cursor != bytes.len() {
        return Err(animation_error(
            "TrackValueMap has trailing bytes after its records",
        ));
    }
    Ok(entries)
}

fn parse_camera_2d_transform(bytes: &[u8], limits: Limits) -> Result<Camera2DTransform> {
    enforce_byte_limit(
        bytes.len() as u64,
        limits.max_animation_bytes(),
        "2D-camera transform snapshot",
    )?;
    let mut cursor = 0;
    let header_size_raw = read_be_u32(bytes, &mut cursor)?;
    let point_record_size_raw = read_be_u32(bytes, &mut cursor)?;
    let header_size = header_size_raw as usize;
    let point_record_size = point_record_size_raw as usize;
    let point_count = read_be_u32(bytes, &mut cursor)?;
    enforce_item_limit(
        u64::from(point_count),
        limits.max_animation_items(),
        "2D-camera transform points",
    )?;
    if header_size < 120 {
        return Err(animation_error(format!(
            "2D-camera transform header size {header_size} is below 120"
        )));
    }
    if point_record_size < 16 {
        return Err(animation_error(format!(
            "2D-camera point record size {point_record_size} is below 16"
        )));
    }
    let expected_size = (point_count as usize)
        .checked_mul(point_record_size)
        .and_then(|size| header_size.checked_add(size))
        .ok_or(Error::OffsetOverflow)?;
    if bytes.len() != expected_size {
        return Err(animation_error(format!(
            "2D-camera transform has {} bytes instead of {expected_size}",
            bytes.len()
        )));
    }
    let mut prefix_words = [0_u32; 5];
    for value in &mut prefix_words {
        *value = read_be_u32(bytes, &mut cursor)?;
    }
    let width = read_be_u32(bytes, &mut cursor)?;
    let height = read_be_u32(bytes, &mut cursor)?;
    if width == 0
        || height == 0
        || width > limits.max_canvas_dimension()
        || height > limits.max_canvas_dimension()
    {
        return Err(animation_error(format!(
            "2D-camera transform dimensions {width}x{height} are invalid"
        )));
    }
    let scale = read_camera_point(bytes, &mut cursor, "scale")?;
    let rotation = read_be_f64(bytes, &mut cursor)?;
    let position = read_camera_point(bytes, &mut cursor, "position")?;
    let image_center = read_camera_point(bytes, &mut cursor, "image center")?;
    if !rotation.is_finite() {
        return Err(animation_error(
            "2D-camera transform rotation is not finite",
        ));
    }
    let mut suffix_words = [0_u32; 6];
    for value in &mut suffix_words {
        *value = read_be_u32(bytes, &mut cursor)?;
    }
    if cursor > header_size {
        return Err(animation_error(
            "2D-camera known header fields exceed its declared header",
        ));
    }
    cursor = header_size;
    let mut corners = Vec::new();
    corners
        .try_reserve_exact(point_count as usize)
        .map_err(|_| Error::LimitExceeded {
            resource: "2D-camera point allocation",
            value: u64::from(point_count),
            limit: limits.max_animation_items(),
        })?;
    for _ in 0..point_count {
        let record_end = cursor
            .checked_add(point_record_size)
            .ok_or(Error::OffsetOverflow)?;
        corners.push(read_camera_point(bytes, &mut cursor, "frame corner")?);
        cursor = record_end;
    }
    Ok(Camera2DTransform {
        header_size: header_size_raw,
        point_record_size: point_record_size_raw,
        width,
        height,
        scale,
        rotation,
        position,
        image_center,
        corners,
        prefix_words,
        suffix_words,
        raw: bytes.into(),
    })
}

fn read_camera_point(bytes: &[u8], cursor: &mut usize, name: &str) -> Result<Camera2DPoint> {
    let point = Camera2DPoint {
        x: read_be_f64(bytes, cursor)?,
        y: read_be_f64(bytes, cursor)?,
    };
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err(animation_error(format!("2D-camera {name} is not finite")));
    }
    Ok(point)
}

fn read_utf16be_value(bytes: &[u8], cursor: &mut usize, limit: usize) -> Result<String> {
    let count = read_be_u32_bounded(bytes, cursor, limit)? as usize;
    let byte_count = count.checked_mul(2).ok_or(Error::OffsetOverflow)?;
    let end = cursor
        .checked_add(byte_count)
        .ok_or(Error::OffsetOverflow)?;
    let encoded = bytes
        .get(*cursor..end)
        .filter(|_| end <= limit)
        .ok_or_else(|| animation_error("truncated UTF-16BE TrackValueMap string"))?;
    let units = encoded
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
    let value = char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| animation_error("TrackValueMap string is invalid UTF-16BE"))?;
    *cursor = end;
    Ok(value)
}

fn read_be_u32_bounded(bytes: &[u8], cursor: &mut usize, limit: usize) -> Result<u32> {
    let end = cursor.checked_add(4).ok_or(Error::OffsetOverflow)?;
    if end > limit {
        return Err(animation_error("truncated TrackValueMap record"));
    }
    read_be_u32(bytes, cursor)
}

fn read_be_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let end = cursor.checked_add(4).ok_or(Error::OffsetOverflow)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| animation_error("truncated big-endian animation integer"))?;
    *cursor = end;
    Ok(u32::from_be_bytes(value.try_into().expect("four bytes")))
}

fn read_be_f64(bytes: &[u8], cursor: &mut usize) -> Result<f64> {
    let end = cursor.checked_add(8).ok_or(Error::OffsetOverflow)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| animation_error("truncated big-endian animation float"))?;
    *cursor = end;
    Ok(f64::from_be_bytes(value.try_into().expect("eight bytes")))
}

struct AnimationCurveHeader {
    cursor: usize,
    kind: String,
    axis: Option<String>,
}

fn animation_curve_header(
    bytes: &[u8],
    start: usize,
    strings: &[String],
    fcurve: u32,
) -> Option<AnimationCurveHeader> {
    let mut cursor = start;
    if read_u32_optional(bytes, &mut cursor)? != fcurve
        || read_u32_optional(bytes, &mut cursor)? != 0
    {
        return None;
    }
    let property_count = read_u32_optional(bytes, &mut cursor)?;
    if !(1..=8).contains(&property_count) {
        return None;
    }
    let mut kind = None;
    let mut axis = None;
    for _ in 0..property_count {
        let property = strings.get(read_u32_optional(bytes, &mut cursor)? as usize)?;
        let value = strings.get(read_u32_optional(bytes, &mut cursor)? as usize)?;
        match property.as_str() {
            "Type" if kind.is_none() => kind = Some(value.clone()),
            "Axis" if axis.is_none() => axis = Some(value.clone()),
            "Type" | "Axis" => return None,
            _ => {}
        }
    }
    Some(AnimationCurveHeader {
        cursor,
        kind: kind?,
        axis,
    })
}

fn read_u32_optional(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let end = cursor.checked_add(4)?;
    let value = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

fn parse_animation_curves(bytes: &[u8], limits: Limits) -> Result<Vec<AnimationCurve>> {
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let Some(fcurve) = string_id_optional(&strings, "FCurve") else {
        return Ok(Vec::new());
    };
    let mut curves = Vec::new();
    for start in data_start..=bytes.len().saturating_sub(12) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        enforce_item_limit(
            curves.len() as u64 + 1,
            limits.max_animation_items(),
            "animation mixer curves",
        )?;
        let mut cursor = header.cursor;
        curves.push(parse_animation_curve_fields(
            bytes,
            &strings,
            &mut cursor,
            header.kind,
            header.axis,
            limits,
        )?);
    }
    Ok(curves)
}

fn parse_secondary_animation_curves(
    bytes: &[u8],
    limits: Limits,
) -> Result<Vec<SecondaryAnimationCurve>> {
    let (strings, data_start) = parse_string_table_with_data_start(bytes, limits)?;
    let Some(fcurve) = string_id_optional(&strings, "FCurve") else {
        return Ok(Vec::new());
    };
    let Some(int32_array) = string_id_optional(&strings, "Int32[]") else {
        return Ok(Vec::new());
    };
    let minimum_size = 12;
    let mut curves = Vec::new();
    for start in data_start..=bytes.len().saturating_sub(minimum_size) {
        let Some(header) = animation_curve_header(bytes, start, &strings, fcurve) else {
            continue;
        };
        let mut cursor = header.cursor;
        let field_count = read_u32(bytes, &mut cursor)?;
        if !secondary_field_header_matches(bytes, cursor, strings.len(), int32_array) {
            continue;
        }
        enforce_item_limit(
            u64::from(field_count),
            limits.max_animation_items().min(1_024),
            "secondary animation mixer fields",
        )?;
        enforce_item_limit(
            curves.len() as u64 + 1,
            limits.max_animation_items(),
            "secondary animation mixer curves",
        )?;
        curves.push(parse_secondary_animation_curve_fields(
            bytes,
            &strings,
            &mut cursor,
            header.kind,
            header.axis,
            field_count,
            limits,
        )?);
    }
    Ok(curves)
}

fn parse_secondary_animation_curve_fields(
    bytes: &[u8],
    strings: &[String],
    cursor: &mut usize,
    kind: String,
    axis: Option<String>,
    field_count: u32,
    limits: Limits,
) -> Result<SecondaryAnimationCurve> {
    let int32_array = string_id_optional(strings, "Int32[]")
        .ok_or_else(|| animation_error("secondary animation mixer lacks \"Int32[]\""))?;
    let mut frames = None;
    let mut values = None;
    let mut tags = None;
    let mut interpolation = None;
    let mut left_slopes = None;
    let mut right_slopes = None;
    let mut revise_constant = None;
    for _ in 0..field_count {
        if !secondary_field_header_matches(bytes, *cursor, strings.len(), int32_array) {
            return Err(animation_error(format!(
                "secondary FCurve {kind} has an invalid field metadata header"
            )));
        }
        *cursor = cursor.checked_add(3 * 4).ok_or(Error::OffsetOverflow)?;
        let field_id = read_u32(bytes, cursor)?;
        let type_id = read_u32(bytes, cursor)?;
        let field = string_at(strings, field_id)?;
        let field_type = string_at(strings, type_id)?;
        let count = read_u32(bytes, cursor)?;
        enforce_item_limit(
            u64::from(count),
            limits.max_animation_items(),
            "secondary animation mixer array items",
        )?;
        let count = count as usize;
        match field_type {
            "Double[]" if matches!(field, "Frame" | "Value" | "LeftSlope" | "RightSlope") => {
                let mut array = Vec::new();
                array
                    .try_reserve_exact(count)
                    .map_err(|_| Error::LimitExceeded {
                        resource: "secondary animation mixer array allocation",
                        value: count as u64,
                        limit: limits.max_animation_items(),
                    })?;
                for _ in 0..count {
                    array.push(f64::from_bits(read_u64_le(bytes, cursor)?));
                }
                match field {
                    "Frame" => frames = Some(array),
                    "Value" => values = Some(array),
                    "LeftSlope" => left_slopes = Some(array),
                    "RightSlope" => right_slopes = Some(array),
                    _ => unreachable!(),
                }
            }
            "Single[]" if matches!(field, "Frame" | "Value" | "LeftSlope" | "RightSlope") => {
                let mut array = Vec::new();
                array
                    .try_reserve_exact(count)
                    .map_err(|_| Error::LimitExceeded {
                        resource: "secondary animation mixer array allocation",
                        value: count as u64,
                        limit: limits.max_animation_items(),
                    })?;
                for _ in 0..count {
                    array.push(f64::from(f32::from_bits(read_u32(bytes, cursor)?)));
                }
                match field {
                    "Frame" => frames = Some(array),
                    "Value" => values = Some(array),
                    "LeftSlope" => left_slopes = Some(array),
                    "RightSlope" => right_slopes = Some(array),
                    _ => unreachable!(),
                }
            }
            "String[]" if field == "Tag" || field == "Interp" => {
                let mut array = Vec::new();
                array
                    .try_reserve_exact(count)
                    .map_err(|_| Error::LimitExceeded {
                        resource: "secondary animation string allocation",
                        value: count as u64,
                        limit: limits.max_animation_items(),
                    })?;
                for _ in 0..count {
                    let value_id = read_u32(bytes, cursor)?;
                    array.push(string_at(strings, value_id)?.to_owned());
                }
                if field == "Tag" {
                    tags = Some(array);
                } else {
                    interpolation = Some(array);
                }
            }
            "Byte[]" if field == "ReviseConstant" => {
                let end = cursor.checked_add(count).ok_or(Error::OffsetOverflow)?;
                revise_constant = Some(
                    bytes
                        .get(*cursor..end)
                        .ok_or_else(|| animation_error("truncated secondary ReviseConstant array"))?
                        .to_vec(),
                );
                *cursor = end;
            }
            "Double[]" => skip_array(bytes, cursor, count, 8)?,
            "Single[]" | "String[]" | "Int32[]" | "UInt32[]" => {
                skip_array(bytes, cursor, count, 4)?;
            }
            "Byte[]" => skip(bytes, cursor, count)?,
            "Float2[]" => skip_array(bytes, cursor, count, 8)?,
            "Float3[]" => skip_array(bytes, cursor, count, 12)?,
            "Double2[]" => skip_array(bytes, cursor, count, 16)?,
            "Double3[]" => skip_array(bytes, cursor, count, 24)?,
            "Quat[]" => skip_array(bytes, cursor, count, 32)?,
            "Matrix44[]" => skip_array(bytes, cursor, count, 128)?,
            other => {
                return Err(animation_error(format!(
                    "unsupported secondary FCurve field type {other:?} for {field:?}"
                )));
            }
        }
        if [read_u32(bytes, cursor)?, read_u32(bytes, cursor)?] != [0, 0] {
            return Err(animation_error(format!(
                "secondary FCurve field {field:?} has a nonzero terminator"
            )));
        }
    }
    let frames =
        frames.ok_or_else(|| animation_error(format!("secondary {kind} has no Frame array")))?;
    let values =
        values.ok_or_else(|| animation_error(format!("secondary {kind} has no Value array")))?;
    let count = frames.len();
    require_curve_array_length(&kind, "secondary Value", values.len(), count)?;
    require_optional_curve_array_length(&kind, "secondary Tag", tags.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "secondary Interp", interpolation.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "secondary LeftSlope", left_slopes.as_ref(), count)?;
    require_optional_curve_array_length(
        &kind,
        "secondary RightSlope",
        right_slopes.as_ref(),
        count,
    )?;
    require_optional_curve_array_length(
        &kind,
        "secondary ReviseConstant",
        revise_constant.as_ref(),
        count,
    )?;
    if frames.iter().any(|value| !value.is_finite())
        || values.iter().any(|value| !value.is_finite())
        || frames.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(animation_error(format!(
            "secondary {kind} curve contains invalid or unsorted numeric values"
        )));
    }
    let mut keyframes = Vec::new();
    keyframes
        .try_reserve_exact(count)
        .map_err(|_| Error::LimitExceeded {
            resource: "secondary animation curve key allocation",
            value: count as u64,
            limit: limits.max_animation_items(),
        })?;
    for (index, (time_60hz, value)) in frames.into_iter().zip(values).enumerate() {
        keyframes.push(SecondaryAnimationCurveKeyframe {
            time_60hz,
            value,
            tag: tags.as_ref().map(|array| array[index].clone()),
            interpolation: interpolation.as_ref().map(|array| array[index].clone()),
            left_slope: left_slopes.as_ref().map(|array| array[index]),
            right_slope: right_slopes.as_ref().map(|array| array[index]),
            revise_constant: revise_constant.as_ref().map(|array| array[index]),
        });
    }
    Ok(SecondaryAnimationCurve {
        kind,
        axis,
        keyframes,
    })
}

fn secondary_field_header_matches(
    bytes: &[u8],
    start: usize,
    string_count: usize,
    int32_array: u32,
) -> bool {
    let mut words = [0_u32; 3];
    for (index, word) in words.iter_mut().enumerate() {
        let offset = match start.checked_add(index * 4) {
            Some(offset) => offset,
            None => return false,
        };
        *word = match bytes
            .get(offset..offset + 4)
            .and_then(|value| <[u8; 4]>::try_from(value).ok())
        {
            Some(value) => u32::from_le_bytes(value),
            None => return false,
        };
    }
    words[0] == int32_array
        && words[1..]
            .iter()
            .all(|value| (*value as usize) < string_count)
}

fn parse_animation_curve_fields(
    bytes: &[u8],
    strings: &[String],
    cursor: &mut usize,
    kind: String,
    axis: Option<String>,
    limits: Limits,
) -> Result<AnimationCurve> {
    let field_count = read_u32(bytes, cursor)?;
    enforce_item_limit(
        u64::from(field_count),
        limits.max_animation_items().min(1_024),
        "animation mixer fields",
    )?;
    let mut frames = None;
    let mut values = None;
    let mut tags = None;
    let mut interpolation = None;
    let mut left_slopes = None;
    let mut right_slopes = None;
    let mut revise_constant = None;
    for _ in 0..field_count {
        let field_id = read_u32(bytes, cursor)?;
        let type_id = read_u32(bytes, cursor)?;
        let field = string_at(strings, field_id)?;
        let field_type = string_at(strings, type_id)?;
        let count = read_u32(bytes, cursor)?;
        enforce_item_limit(
            u64::from(count),
            limits.max_animation_items(),
            "animation mixer array items",
        )?;
        let count = count as usize;
        match field_type {
            "Single[]" if matches!(field, "Frame" | "Value" | "LeftSlope" | "RightSlope") => {
                let mut array = Vec::new();
                array
                    .try_reserve_exact(count)
                    .map_err(|_| Error::LimitExceeded {
                        resource: "animation mixer array allocation",
                        value: count as u64,
                        limit: limits.max_animation_items(),
                    })?;
                for _ in 0..count {
                    array.push(f32::from_bits(read_u32(bytes, cursor)?));
                }
                match field {
                    "Frame" => frames = Some(array),
                    "Value" => values = Some(array),
                    "LeftSlope" => left_slopes = Some(array),
                    "RightSlope" => right_slopes = Some(array),
                    _ => unreachable!(),
                }
            }
            "String[]" if field == "Tag" || field == "Interp" => {
                let mut array = Vec::new();
                array
                    .try_reserve_exact(count)
                    .map_err(|_| Error::LimitExceeded {
                        resource: "animation tag allocation",
                        value: count as u64,
                        limit: limits.max_animation_items(),
                    })?;
                for _ in 0..count {
                    let value_id = read_u32(bytes, cursor)?;
                    array.push(string_at(strings, value_id)?.to_owned());
                }
                if field == "Tag" {
                    tags = Some(array);
                } else {
                    interpolation = Some(array);
                }
            }
            "Byte[]" if field == "ReviseConstant" => {
                let end = cursor.checked_add(count).ok_or(Error::OffsetOverflow)?;
                revise_constant = Some(
                    bytes
                        .get(*cursor..end)
                        .ok_or_else(|| animation_error("truncated ReviseConstant array"))?
                        .to_vec(),
                );
                *cursor = end;
            }
            "Single[]" | "String[]" | "Int32[]" => {
                skip_array(bytes, cursor, count, 4)?;
            }
            "Byte[]" => skip(bytes, cursor, count)?,
            "Float2[]" => skip_array(bytes, cursor, count, 8)?,
            "Float3[]" => skip_array(bytes, cursor, count, 12)?,
            "Quat[]" => skip_array(bytes, cursor, count, 16)?,
            "Matrix44[]" => skip_array(bytes, cursor, count, 64)?,
            other => {
                return Err(animation_error(format!(
                    "unsupported FCurve field type {other:?} for {field:?}"
                )));
            }
        }
        if [read_u32(bytes, cursor)?, read_u32(bytes, cursor)?] != [0, 0] {
            return Err(animation_error(format!(
                "FCurve field {field:?} has a nonzero terminator"
            )));
        }
    }
    let frames = frames.ok_or_else(|| animation_error(format!("{kind} has no Frame array")))?;
    let values = values.ok_or_else(|| animation_error(format!("{kind} has no Value array")))?;
    let count = frames.len();
    require_curve_array_length(&kind, "Value", values.len(), count)?;
    require_optional_curve_array_length(&kind, "Tag", tags.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "Interp", interpolation.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "LeftSlope", left_slopes.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "RightSlope", right_slopes.as_ref(), count)?;
    require_optional_curve_array_length(&kind, "ReviseConstant", revise_constant.as_ref(), count)?;
    if frames.iter().any(|value| !value.is_finite())
        || values.iter().any(|value| !value.is_finite())
        || frames.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(animation_error(format!(
            "{kind} curve contains invalid or unsorted numeric values"
        )));
    }
    let mut keyframes = Vec::new();
    keyframes
        .try_reserve_exact(count)
        .map_err(|_| Error::LimitExceeded {
            resource: "animation curve key allocation",
            value: count as u64,
            limit: limits.max_animation_items(),
        })?;
    for (index, (time_60hz, value)) in frames.into_iter().zip(values).enumerate() {
        keyframes.push(AnimationCurveKeyframe {
            time_60hz,
            value,
            tag: tags.as_ref().map(|array| array[index].clone()),
            interpolation: interpolation.as_ref().map(|array| array[index].clone()),
            left_slope: left_slopes.as_ref().map(|array| array[index]),
            right_slope: right_slopes.as_ref().map(|array| array[index]),
            revise_constant: revise_constant.as_ref().map(|array| array[index]),
        });
    }
    Ok(AnimationCurve {
        kind,
        axis,
        keyframes,
    })
}

fn require_curve_array_length(
    curve: &str,
    field: &str,
    actual: usize,
    expected: usize,
) -> Result<()> {
    if actual != expected {
        return Err(animation_error(format!(
            "{curve} {field} length {actual} differs from Frame length {expected}"
        )));
    }
    Ok(())
}

fn require_optional_curve_array_length<T>(
    curve: &str,
    field: &str,
    values: Option<&Vec<T>>,
    expected: usize,
) -> Result<()> {
    if let Some(values) = values {
        require_curve_array_length(curve, field, values.len(), expected)?;
    }
    Ok(())
}

fn parse_string_table_with_data_start(
    bytes: &[u8],
    limits: Limits,
) -> Result<(Vec<String>, usize)> {
    if bytes.len() < 20 || !matches!(&bytes[..12], b"cmt 0100binc" | b"cmt 0110binc") {
        return Err(animation_error("unsupported animation mixer signature"));
    }
    let mut cursor = 16;
    let count = read_u32(bytes, &mut cursor)?;
    enforce_item_limit(
        u64::from(count),
        limits.max_animation_items(),
        "animation mixer strings",
    )?;
    let mut strings = Vec::new();
    strings
        .try_reserve_exact(count as usize)
        .map_err(|_| Error::LimitExceeded {
            resource: "animation mixer string allocation",
            value: u64::from(count),
            limit: limits.max_animation_items(),
        })?;
    for _ in 0..count {
        let length = *bytes
            .get(cursor)
            .ok_or_else(|| animation_error("truncated animation mixer string length"))?
            as usize;
        cursor += 1;
        let end = cursor.checked_add(length).ok_or(Error::OffsetOverflow)?;
        let value = bytes
            .get(cursor..end)
            .ok_or_else(|| animation_error("truncated animation mixer string"))?;
        strings.push(
            str::from_utf8(value)
                .map_err(|_| animation_error("animation mixer string is not UTF-8"))?
                .to_owned(),
        );
        cursor = end;
    }
    Ok((strings, cursor))
}

fn string_id_optional(strings: &[String], wanted: &str) -> Option<u32> {
    strings
        .iter()
        .position(|value| value == wanted)
        .and_then(|index| u32::try_from(index).ok())
}

fn string_at(strings: &[String], index: u32) -> Result<&str> {
    strings
        .get(index as usize)
        .map(String::as_str)
        .ok_or_else(|| animation_error(format!("animation string index {index} is out of range")))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let end = cursor.checked_add(4).ok_or(Error::OffsetOverflow)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| animation_error("truncated animation mixer integer"))?;
    *cursor = end;
    Ok(u32::from_le_bytes(value.try_into().expect("four bytes")))
}

fn read_u64_le(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let end = cursor.checked_add(8).ok_or(Error::OffsetOverflow)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| animation_error("truncated secondary animation mixer integer"))?;
    *cursor = end;
    Ok(u64::from_le_bytes(value.try_into().expect("eight bytes")))
}

fn skip_array(bytes: &[u8], cursor: &mut usize, count: usize, element_size: usize) -> Result<()> {
    let length = count
        .checked_mul(element_size)
        .ok_or(Error::OffsetOverflow)?;
    skip(bytes, cursor, length)
}

fn skip(bytes: &[u8], cursor: &mut usize, length: usize) -> Result<()> {
    let end = cursor.checked_add(length).ok_or(Error::OffsetOverflow)?;
    bytes
        .get(*cursor..end)
        .ok_or_else(|| animation_error("truncated animation mixer array"))?;
    *cursor = end;
    Ok(())
}

fn optional_text<'a>(
    value: ValueRef<'a>,
    column: usize,
    name: &str,
) -> rusqlite::Result<Option<&'a str>> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => str::from_utf8(bytes)
            .map(Some)
            .map_err(|error| rusqlite::Error::Utf8Error(column, error)),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
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

fn required_bytes<'a>(
    value: ValueRef<'a>,
    column: usize,
    name: &str,
) -> rusqlite::Result<&'a [u8]> {
    match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(bytes),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn enforce_item_limit(value: u64, limit: u64, resource: &'static str) -> Result<()> {
    if value > limit {
        return Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        });
    }
    Ok(())
}

fn enforce_byte_limit(value: u64, limit: u64, resource: &'static str) -> Result<()> {
    if value > limit {
        return Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        });
    }
    Ok(())
}

fn animation_error(reason: impl Into<String>) -> Error {
    Error::InvalidAnimation {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::ZlibEncoder};
    use rusqlite::Connection;
    #[cfg(feature = "write")]
    use rusqlite::MAIN_DB;

    use super::*;

    const IDENTIFIER: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";
    #[cfg(feature = "write")]
    const SECONDARY_IDENTIFIER: &[u8] = b"secondary0123456789ABCDEF0123456789ABCDEF";
    const LAYER_UUID: [u8; 16] = [0x11; 16];

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_be_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_utf16be(bytes: &mut Vec<u8>, value: &str) {
        let encoded = value.encode_utf16().collect::<Vec<_>>();
        push_be_u32(bytes, encoded.len() as u32);
        for unit in encoded {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
    }

    fn push_value_record(bytes: &mut Vec<u8>, name: &str, text: &str, kind: u32, payload: &[u8]) {
        let start = bytes.len();
        push_be_u32(bytes, 0);
        push_utf16be(bytes, name);
        push_utf16be(bytes, text);
        push_be_u32(bytes, kind);
        bytes.extend_from_slice(payload);
        let size = u32::try_from(bytes.len() - start).unwrap();
        bytes[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    fn track_value_map() -> Vec<u8> {
        let mut bytes = Vec::new();
        push_be_u32(&mut bytes, 8);
        push_be_u32(&mut bytes, 1);
        push_value_record(&mut bytes, "ImageCelName", "A", 2, &0_u32.to_be_bytes());
        bytes
    }

    fn axis_binc() -> Vec<u8> {
        let strings = [
            "FCurve",
            "Type",
            "ImagePosition",
            "Axis",
            "X",
            "Frame",
            "Single[]",
            "Value",
            "ReviseConstant",
            "Byte[]",
        ];
        let mut bytes = Vec::from(b"cmt 0100binc".as_slice());
        bytes.extend_from_slice(&[0; 4]);
        push_u32(&mut bytes, strings.len() as u32);
        for value in strings {
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value.as_bytes());
        }
        for value in [0, 0, 2, 1, 2, 3, 4, 3] {
            push_u32(&mut bytes, value);
        }
        for (field, value) in [(5, 94.0_f32), (7, 447.0)] {
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 6);
            push_u32(&mut bytes, 1);
            push_u32(&mut bytes, value.to_bits());
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        push_u32(&mut bytes, 8);
        push_u32(&mut bytes, 9);
        push_u32(&mut bytes, 1);
        bytes.push(1);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes
    }

    fn secondary_axis_binc() -> Vec<u8> {
        let strings = [
            "FCurve",
            "Type",
            "ImagePosition",
            "Axis",
            "X",
            "Int32[]",
            "Name",
            "End",
            "Frame",
            "Double[]",
            "Value",
            "ReviseConstant",
            "Byte[]",
        ];
        let mut bytes = Vec::from(b"cmt 0110binc".as_slice());
        bytes.extend_from_slice(&[0; 4]);
        push_u32(&mut bytes, strings.len() as u32);
        for value in strings {
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value.as_bytes());
        }
        for value in [0, 0, 2, 1, 2, 3, 4, 3] {
            push_u32(&mut bytes, value);
        }
        for (field, value) in [(8, 94.0_f64), (10, 447.0)] {
            for prefix in [5, 1, 2] {
                push_u32(&mut bytes, prefix);
            }
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 9);
            push_u32(&mut bytes, 1);
            bytes.extend_from_slice(&value.to_le_bytes());
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        for prefix in [5, 1, 2] {
            push_u32(&mut bytes, prefix);
        }
        push_u32(&mut bytes, 11);
        push_u32(&mut bytes, 12);
        push_u32(&mut bytes, 1);
        bytes.push(1);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes
    }

    fn camera_transform() -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [120_u32, 16, 4, 0, 0, 0, 2, 2, 720, 540] {
            push_be_u32(&mut bytes, value);
        }
        for value in [1.0_f64, 1.0, 0.0, 447.0, 327.0, 360.0, 270.0] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        for value in [3_u32, 1, 0, 1, 0, 0] {
            push_be_u32(&mut bytes, value);
        }
        for (x, y) in [
            (87.0_f64, 57.0_f64),
            (807.0, 57.0),
            (87.0, 597.0),
            (807.0, 597.0),
        ] {
            bytes.extend_from_slice(&x.to_be_bytes());
            bytes.extend_from_slice(&y.to_be_bytes());
        }
        bytes
    }

    fn binc() -> Vec<u8> {
        let strings = [
            "FCurve",
            "Type",
            "ImageCelName",
            "Frame",
            "Single[]",
            "Value",
            "Tag",
            "String[]",
            "A",
            "B",
            "Interp",
            "LeftSlope",
            "RightSlope",
            "ReviseConstant",
            "Byte[]",
            "Linear",
        ];
        let mut bytes = Vec::from(b"cmt 0100binc".as_slice());
        bytes.extend_from_slice(&[0; 4]);
        push_u32(&mut bytes, strings.len() as u32);
        for value in strings {
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value.as_bytes());
        }
        for value in [0, 0, 1, 1, 2] {
            push_u32(&mut bytes, value);
        }
        push_u32(&mut bytes, 7);

        for (field, kind, values) in [(3, 4, [0.0_f32, 60.0]), (5, 4, [0.0_f32, 1.0])] {
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, kind);
            push_u32(&mut bytes, values.len() as u32);
            for value in values {
                push_u32(&mut bytes, value.to_bits());
            }
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        push_u32(&mut bytes, 6);
        push_u32(&mut bytes, 7);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 8);
        push_u32(&mut bytes, 9);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 10);
        push_u32(&mut bytes, 7);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 15);
        push_u32(&mut bytes, 15);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        for field in [11, 12] {
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 4);
            push_u32(&mut bytes, 2);
            push_u32(&mut bytes, 0.0_f32.to_bits());
            push_u32(&mut bytes, 0.0_f32.to_bits());
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        push_u32(&mut bytes, 13);
        push_u32(&mut bytes, 14);
        push_u32(&mut bytes, 2);
        bytes.extend_from_slice(&[1, 0]);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes
    }

    fn secondary_binc() -> Vec<u8> {
        let strings = [
            "FCurve",
            "Type",
            "ImageCelName",
            "Int32[]",
            "Name",
            "End",
            "Frame",
            "Double[]",
            "Value",
            "Tag",
            "String[]",
            "A",
            "B",
            "Interp",
            "Linear",
            "LeftSlope",
            "RightSlope",
            "ReviseConstant",
            "Byte[]",
            "Version",
            "Version-Information",
            "2.1.0",
        ];
        let mut bytes = Vec::from(b"cmt 0110binc".as_slice());
        bytes.extend_from_slice(&[0; 4]);
        push_u32(&mut bytes, strings.len() as u32);
        for value in strings {
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value.as_bytes());
        }

        for value in [0, 0, 1, 1, 2, 7, 3, 99, 100] {
            push_u32(&mut bytes, value);
        }
        for value in [0, 0, 1, 1, 2] {
            push_u32(&mut bytes, value);
        }
        push_u32(&mut bytes, 7);

        let prefix = [3, 4, 5];
        for (field, values) in [(6, [0.0_f64, 60.0]), (8, [0.0_f64, 1.0])] {
            for value in prefix {
                push_u32(&mut bytes, value);
            }
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 7);
            push_u32(&mut bytes, values.len() as u32);
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        let string_prefix = [3, 19, 4];
        for (field, values) in [(9, [11, 12]), (13, [14, 14])] {
            for value in string_prefix {
                push_u32(&mut bytes, value);
            }
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 10);
            push_u32(&mut bytes, values.len() as u32);
            for value in values {
                push_u32(&mut bytes, value);
            }
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        for field in [15, 16] {
            for value in prefix {
                push_u32(&mut bytes, value);
            }
            push_u32(&mut bytes, field);
            push_u32(&mut bytes, 7);
            push_u32(&mut bytes, 2);
            bytes.extend_from_slice(&0.0_f64.to_le_bytes());
            bytes.extend_from_slice(&0.0_f64.to_le_bytes());
            push_u32(&mut bytes, 0);
            push_u32(&mut bytes, 0);
        }
        let byte_prefix = [3, 20, 21];
        for value in byte_prefix {
            push_u32(&mut bytes, value);
        }
        push_u32(&mut bytes, 17);
        push_u32(&mut bytes, 18);
        push_u32(&mut bytes, 2);
        bytes.extend_from_slice(&[1, 0]);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes
    }

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

    fn animation_database(external_offset: u64) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE TimeLine (
                    MainId INTEGER, BankId INTEGER, NextTimeLine INTEGER,
                    FirstTrack INTEGER, TimeLineName TEXT, FrameRate REAL,
                    StartFrame REAL, EndFrame REAL, CurrentFrame REAL
                 );
                 INSERT INTO TimeLine VALUES (1, 2, NULL, 1, NULL, 24, 0, 24, 0);
                 CREATE TABLE AnimationCutBank (
                    MainId INTEGER, FirstTimeLine INTEGER, Enable INTEGER
                 );
                 INSERT INTO AnimationCutBank VALUES (1, 1, 1);
                 CREATE TABLE Track (
                    MainId INTEGER, BankId INTEGER, TrackKind INTEGER,
                    TrackActionMixer BLOB, LayerUuidWithTrack BLOB,
                    TrackActionMixer2 BLOB, TrackValueMap BLOB,
                    TrackNextIndex INTEGER
                 );
                 CREATE TABLE Layer (MainId INTEGER, LayerUuid TEXT);
                 INSERT INTO Layer VALUES (5, '11111111-1111-1111-1111-111111111111');
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Track VALUES (1, 2, 2000, ?1, ?2, ?1, ?3, 0)",
                params![IDENTIFIER, LAYER_UUID, track_value_map()],
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
    fn writable_animation_sample() -> Vec<u8> {
        fn compressed_body(mixer: &[u8]) -> Vec<u8> {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(mixer).unwrap();
            let compressed = encoder.finish().unwrap();
            let mut body = Vec::with_capacity(compressed.len() + 4);
            body.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            body.extend_from_slice(&compressed);
            body
        }

        let primary_mixer = binc();
        let secondary_mixer = secondary_binc();
        let primary_body = compressed_body(&primary_mixer);
        let secondary_body = compressed_body(&secondary_mixer);
        let first_external_offset = 24 + 16 + 40;
        let first_external_size = 16 + 16 + IDENTIFIER.len() as u64 + primary_body.len() as u64;
        let second_external_offset = first_external_offset + first_external_size;
        let second_external_size =
            16 + 16 + SECONDARY_IDENTIFIER.len() as u64 + secondary_body.len() as u64;
        let database_offset = second_external_offset + second_external_size;

        let mut value_map = Vec::new();
        push_be_u32(&mut value_map, 8);
        push_be_u32(&mut value_map, 2);
        push_value_record(&mut value_map, "ImageCelName", "A", 2, &0_u32.to_be_bytes());
        push_value_record(
            &mut value_map,
            "Opacity",
            "",
            0,
            &100.0_f64.to_bits().to_be_bytes(),
        );

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE TimeLine (
                    MainId INTEGER, BankId INTEGER, NextTimeLine INTEGER,
                    FirstTrack INTEGER, TimeLineName TEXT, FrameRate REAL,
                    StartFrame REAL, EndFrame REAL, CurrentFrame REAL
                 );
                 INSERT INTO TimeLine VALUES (1, 2, NULL, 1, NULL, 24, 0, 24, 0);
                 CREATE TABLE AnimationCutBank (
                    MainId INTEGER, FirstTimeLine INTEGER, Enable INTEGER
                 );
                 INSERT INTO AnimationCutBank VALUES (1, 1, 1);
                 CREATE TABLE Track (
                    _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT,
                    MainId INTEGER, BankId INTEGER, ItemId INTEGER,
                    TrackNextIndex INTEGER, TrackActionMixerSize INTEGER,
                    TrackActionMixer BLOB, TrackActionMixer2Size INTEGER,
                    TrackActionMixer2 BLOB, TrackValueMap BLOB,
                    TrackOpen INTEGER, TrackContentOpen INTEGER,
                    TrackUuid BLOB, LayerUuidWithTrack BLOB,
                    TrackKind INTEGER, TrackOptionFlag INTEGER,
                    OpaqueColumn BLOB
                 );
                 CREATE TABLE Layer (
                     MainId INTEGER, LayerUuid TEXT, LayerType INTEGER,
                     LayerFolder INTEGER, AnimationFolder INTEGER, LayerFirstChildIndex INTEGER,
                     LayerNextIndex INTEGER, LayerName TEXT
                  );
                  INSERT INTO Layer VALUES
                     (5, '11111111-1111-1111-1111-111111111111', 0, 17, 1, 8, 0, 'template'),
                     (6, '22222222-2222-2222-2222-222222222222', 1, 0, 0, 0, 0, 'plain'),
                     (7, '44444444-4444-4444-4444-444444444444', 0, 17, 1, 8, 0, 'target'),
                     (8, '55555555-5555-5555-5555-555555555555', 1, 0, 0, 0, 9, 'A'),
                     (9, '66666666-6666-6666-6666-666666666666', 1, 0, 0, 0, 0, 'B');
                 CREATE TABLE ElemScheme (TableName TEXT, MaxIndex INTEGER);
                 INSERT INTO ElemScheme VALUES ('Track', 1);
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Track (
                    MainId, BankId, ItemId, TrackNextIndex,
                    TrackActionMixerSize, TrackActionMixer,
                    TrackActionMixer2Size, TrackActionMixer2, TrackValueMap,
                    TrackOpen, TrackContentOpen, TrackUuid,
                    LayerUuidWithTrack, TrackKind, TrackOptionFlag, OpaqueColumn
                 ) VALUES (
                    1, 2, 1, 0, ?5, ?1, ?6, ?3, ?4,
                    0, 0, x'33333333333333333333333333333333',
                    ?2, 2000, 768, x'DEADBEEF'
                 )",
                params![
                    IDENTIFIER,
                    LAYER_UUID,
                    SECONDARY_IDENTIFIER,
                    value_map,
                    primary_mixer.len() as i64,
                    secondary_mixer.len() as i64,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2), (?3, ?4)",
                params![
                    std::str::from_utf8(IDENTIFIER).unwrap(),
                    first_external_offset as i64,
                    std::str::from_utf8(SECONDARY_IDENTIFIER).unwrap(),
                    second_external_offset as i64,
                ],
            )
            .unwrap();
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        push_u64(&mut header, database_offset);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);

        let mut primary_external = Vec::new();
        push_u64(&mut primary_external, IDENTIFIER.len() as u64);
        primary_external.extend_from_slice(IDENTIFIER);
        push_u64(&mut primary_external, primary_body.len() as u64);
        primary_external.extend_from_slice(&primary_body);
        let mut secondary_external = Vec::new();
        push_u64(&mut secondary_external, SECONDARY_IDENTIFIER.len() as u64);
        secondary_external.extend_from_slice(SECONDARY_IDENTIFIER);
        push_u64(&mut secondary_external, secondary_body.len() as u64);
        secondary_external.extend_from_slice(&secondary_body);

        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);
        assert_eq!(push_chunk(&mut bytes, b"CHNKHead", &header), 24);
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKExta", &primary_external),
            first_external_offset
        );
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKExta", &secondary_external),
            second_external_offset
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

    #[test]
    fn loads_timeline_track_and_cel_curve() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&binc()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        body.extend_from_slice(&compressed);
        let (bytes, offset) = sample(&body);
        let database = animation_database(offset);
        let timelines = database.timelines(Limits::default()).unwrap();
        assert_eq!(timelines.len(), 1);
        assert_eq!(timelines[0].frame_rate(), 24.0);

        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        let animation = clip
            .read_animation(&database, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(animation.timeline().id(), 1);
        assert_eq!(animation.tracks().len(), 1);
        assert_eq!(animation.animation_tracks().len(), 1);
        let raw_track = &animation.animation_tracks()[0];
        assert!(raw_track.kind().is_image_cel());
        assert_eq!(raw_track.layer_id(), Some(5));
        assert_eq!(raw_track.next_track_id(), None);
        assert_eq!(raw_track.curves().len(), 1);
        assert_eq!(raw_track.curves()[0].kind(), "ImageCelName");
        assert_eq!(raw_track.curves()[0].keyframes().len(), 2);
        assert!(raw_track.secondary_action_mixer_present());
        assert!(raw_track.value_map_present());
        assert_eq!(raw_track.values().len(), 1);
        assert_eq!(raw_track.values()[0].name(), "ImageCelName");
        assert_eq!(
            raw_track.values()[0].value(),
            &AnimationTrackValue::IndexedText {
                text: "A".to_owned(),
                numeric_value: 0,
            }
        );
        assert_eq!(raw_track.curves()[0].keyframes()[0].tag(), Some("A"));
        assert_eq!(
            raw_track.curves()[0].keyframes()[0].interpolation(),
            Some("Linear")
        );
        assert_eq!(raw_track.curves()[0].keyframes()[0].left_slope(), Some(0.0));
        assert_eq!(
            raw_track.curves()[0].keyframes()[0].revise_constant(),
            Some(1)
        );
        let track = animation.track_for_layer(5).unwrap();
        assert_eq!(track.keyframes().len(), 2);
        assert_eq!(track.cel_at_frame(0.0, 24.0), Some("A"));
        assert_eq!(track.cel_at_frame(23.0, 24.0), Some("A"));
        assert_eq!(track.cel_at_frame(24.0, 24.0), Some("B"));

        let explicit = clip
            .read_animation_for_timeline(&database, 1, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(explicit.timeline().id(), 1);
        assert!(
            clip.read_animation_for_timeline(&database, 999, Limits::default())
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn high_level_animation_builders_preserve_internal_fields() {
        let template = AnimationCurveKeyframe {
            time_60hz: 0.0,
            value: 1.0,
            tag: Some("A".to_owned()),
            interpolation: Some("Linear".to_owned()),
            left_slope: Some(2.0),
            right_slope: Some(3.0),
            revise_constant: Some(1),
        };
        let insertion = AnimationCurveKeyframeInsert::from_template(&template, 60.0, 4.0);
        assert_eq!(insertion.time_60hz, 60.0);
        assert_eq!(insertion.value, 4.0);
        assert_eq!(insertion.tag.as_deref(), Some("A"));
        assert_eq!(insertion.interpolation.as_deref(), Some("Linear"));
        assert_eq!(insertion.left_slope, Some(2.0));
        assert_eq!(insertion.right_slope, Some(3.0));
        assert_eq!(insertion.revise_constant, Some(1));

        let options =
            ImageCelTrackCloneOptions::from_timed_cels([(0.0, "A"), (30.0, "B"), (60.0, "A")])
                .unwrap();
        assert_eq!(
            options
                .keyframes()
                .iter()
                .map(ImageCelTrackKeyframe::numeric_value)
                .collect::<Vec<_>>(),
            [0, 1, 0]
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn writes_typed_animation_changes_and_reads_them_back() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert_eq!(
            writer
                .replace_animation_cel_tag(1, 0, "new cel", Limits::default())
                .unwrap(),
            "A"
        );
        let original = writer
            .replace_animation_curve_keyframe_numeric(
                1,
                "ImageCelName",
                None,
                1,
                AnimationCurveKeyframeValues::new(120.0, 2.0),
                Limits::default(),
            )
            .unwrap();
        assert_eq!(original.time_60hz(), 60.0);
        assert_eq!(
            writer
                .database()
                .replace_animation_track_value(
                    1,
                    "Opacity",
                    AnimationTrackValue::Float(75.0),
                    Limits::default(),
                )
                .unwrap(),
            AnimationTrackValue::Float(100.0)
        );

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        rewritten.validate().unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let animation = rewritten
            .read_animation(&database, Limits::default())
            .unwrap()
            .unwrap();
        let track = &animation.animation_tracks()[0];
        assert_eq!(track.curves()[0].keyframes()[0].tag(), Some("new cel"));
        assert_eq!(track.curves()[0].keyframes()[1].time_60hz(), 120.0);
        assert_eq!(track.curves()[0].keyframes()[1].value(), 2.0);
        assert_eq!(
            track.secondary_curves()[0].keyframes()[0].tag(),
            Some("new cel")
        );
        assert_eq!(
            track.secondary_curves()[0].keyframes()[1].time_60hz(),
            120.0
        );
        assert_eq!(track.secondary_curves()[0].keyframes()[1].value(), 2.0);
        assert_eq!(
            track
                .values()
                .iter()
                .find(|entry| entry.name() == "ImageCelName")
                .unwrap()
                .value(),
            &AnimationTrackValue::IndexedText {
                text: "new cel".to_owned(),
                numeric_value: 0,
            }
        );
        assert_eq!(
            track
                .values()
                .iter()
                .find(|entry| entry.name() == "Opacity")
                .unwrap()
                .value(),
            &AnimationTrackValue::Float(75.0)
        );

        let (primary_size, secondary_size): (i64, i64) = database
            .connection()
            .query_row(
                "SELECT TrackActionMixerSize, TrackActionMixer2Size \
                 FROM Track WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(primary_size > binc().len() as i64);
        assert!(secondary_size > secondary_binc().len() as i64);
    }

    #[cfg(feature = "write")]
    #[test]
    fn inserts_and_removes_primary_and_secondary_curve_keys() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        let insertion = AnimationCurveKeyframeInsert::new(30.0, 2.0)
            .with_tag("C")
            .with_interpolation("Linear")
            .with_slopes(0.0, 0.0)
            .with_revise_constant(1);

        let inserted = writer
            .insert_animation_curve_keyframe(
                1,
                "ImageCelName",
                None,
                1,
                &insertion,
                Limits::default(),
            )
            .unwrap();
        assert_eq!(inserted.time_60hz(), 30.0);
        assert_eq!(inserted.tag(), Some("C"));
        let removed = writer
            .remove_animation_curve_keyframe(1, "ImageCelName", None, 1, Limits::default())
            .unwrap();
        assert_eq!(removed.time_60hz(), 30.0);
        assert_eq!(removed.tag(), Some("C"));

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        let animation = rewritten
            .read_animation(&database, Limits::default())
            .unwrap()
            .unwrap();
        let track = &animation.animation_tracks()[0];
        assert_eq!(track.curves()[0].keyframes().len(), 2);
        assert_eq!(track.secondary_curves()[0].keyframes().len(), 2);
        assert_eq!(
            track
                .values()
                .iter()
                .find(|entry| entry.name() == "ImageCelName")
                .unwrap()
                .value(),
            &AnimationTrackValue::IndexedText {
                text: "A".to_owned(),
                numeric_value: 0,
            }
        );
        let (primary_size, secondary_size): (i64, i64) = database
            .connection()
            .query_row(
                "SELECT TrackActionMixerSize, TrackActionMixer2Size FROM Track WHERE MainId = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(primary_size >= binc().len() as i64);
        assert!(secondary_size >= secondary_binc().len() as i64);
    }

    #[cfg(feature = "write")]
    #[test]
    fn clones_and_normalizes_an_image_cel_track() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        let options = ImageCelTrackCloneOptions::new([
            ImageCelTrackKeyframe::new(0.0, 0, "A"),
            ImageCelTrackKeyframe::new(30.0, 1, "B"),
        ]);

        let summary = writer
            .clone_image_cel_track_from_template(1, 1, 7, &options, Limits::default())
            .unwrap();
        assert_eq!(summary.track_id(), 2);
        assert_eq!(writer.addition_count(), 2);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        rewritten.validate().unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let animation = rewritten
            .read_animation(&database, Limits::default())
            .unwrap()
            .unwrap();
        let track = animation.track_for_layer(7).unwrap();
        assert_eq!(track.keyframes().len(), 2);
        assert_eq!(track.keyframes()[0].tag(), "A");
        assert_eq!(track.keyframes()[1].time_60hz(), 30.0);
        assert_eq!(track.keyframes()[1].tag(), "B");
    }

    #[cfg(feature = "write")]
    #[test]
    fn checks_image_cel_numeric_f32_boundaries_without_saturating_casts() {
        assert!(u32_is_exactly_representable_as_f32(0));
        assert!(u32_is_exactly_representable_as_f32(16_777_216));
        assert!(!u32_is_exactly_representable_as_f32(16_777_217));
        assert!(u32_is_exactly_representable_as_f32(u32::MAX - 255));
        assert!(!u32_is_exactly_representable_as_f32(u32::MAX));
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_image_cel_clone_for_a_non_animation_folder() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute("UPDATE Layer SET AnimationFolder = 0 WHERE MainId = 7", [])
            .unwrap();
        let options = ImageCelTrackCloneOptions::new([
            ImageCelTrackKeyframe::new(0.0, 0, "A"),
            ImageCelTrackKeyframe::new(30.0, 1, "B"),
        ]);

        assert!(matches!(
            writer.clone_image_cel_track_from_template(1, 1, 7, &options, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_inconsistent_primary_and_secondary_curve_keys() {
        let primary = binc();
        let secondary = secondary_binc();
        assert!(
            validate_matching_secondary_curve_keys(
                &primary,
                &secondary,
                "ImageCelName",
                None,
                Limits::default(),
            )
            .unwrap()
        );

        let mut count_mismatch = secondary.clone();
        assert!(
            edit_secondary_curve_key(
                &mut count_mismatch,
                "ImageCelName",
                None,
                &CurveKeyEdit::Remove { index: 1 },
                Limits::default(),
            )
            .unwrap()
        );
        assert!(matches!(
            validate_matching_secondary_curve_keys(
                &primary,
                &count_mismatch,
                "ImageCelName",
                None,
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));

        let mut time_mismatch = secondary.clone();
        patch_secondary_curve_numeric(
            &mut time_mismatch,
            "ImageCelName",
            None,
            0,
            1.0,
            0.0,
            Limits::default(),
        )
        .unwrap();
        assert!(matches!(
            validate_matching_secondary_curve_keys(
                &primary,
                &time_mismatch,
                "ImageCelName",
                None,
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));

        let mut tag_mismatch = secondary;
        patch_secondary_curve_tag(&mut tag_mismatch, 0, "B", Limits::default()).unwrap();
        assert!(matches!(
            validate_matching_secondary_curve_keys(
                &primary,
                &tag_mismatch,
                "ImageCelName",
                None,
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn rolls_back_image_cel_clone_when_normalization_fails() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        let options = ImageCelTrackCloneOptions::new([ImageCelTrackKeyframe::new(30.0, 0, "A")]);

        assert!(matches!(
            writer.clone_image_cel_track_from_template(1, 1, 7, &options, Limits::default(),),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.addition_count(), 0);
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row("SELECT count(*) FROM Track", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT TrackNextIndex FROM Track WHERE MainId = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT MaxIndex FROM ElemScheme WHERE TableName = 'Track'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_incomplete_key_insertions_without_pending_changes() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.insert_animation_curve_keyframe(
                1,
                "ImageCelName",
                None,
                1,
                &AnimationCurveKeyframeInsert::new(30.0, 2.0),
                Limits::default(),
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(writer.addition_count(), 0);
        assert!(
            writer
                .remove_animation_curve_keyframe(1, "ImageCelName", None, 1, Limits::default(),)
                .is_ok()
        );
        assert!(matches!(
            writer.remove_animation_curve_keyframe(1, "ImageCelName", None, 0, Limits::default(),),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_track_and_repairs_the_timeline_chain() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .clone_animation_track_from_template(1, 1, 6, Limits::default())
            .unwrap();

        let removed = writer
            .remove_animation_track(1, 1, Limits::default())
            .unwrap();
        assert_eq!(removed.track_id(), 1);
        assert_eq!(removed.previous_track_id(), None);
        assert_eq!(removed.next_track_id(), Some(2));
        assert!(removed.retained_primary_mixer_identifier().is_some());
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT FirstTrack FROM TimeLine WHERE MainId = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row("SELECT count(*) FROM Track", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT MaxIndex FROM ElemScheme WHERE TableName = 'Track'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_animation_item_limits_before_clone_or_removal_chain_changes() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_animation_track_from_template(
                1,
                1,
                6,
                Limits::default().with_max_animation_items(1),
            ),
            Err(Error::LimitExceeded {
                resource: "animation tracks after clone",
                value: 2,
                limit: 1,
            })
        ));
        assert_eq!(writer.addition_count(), 0);
        assert!(matches!(
            writer.remove_animation_track(1, 1, Limits::default().with_max_animation_items(0),),
            Err(Error::LimitExceeded {
                resource: "animation timeline tracks",
                value: 1,
                limit: 0,
            })
        ));
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row("SELECT count(*) FROM Track", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT FirstTrack FROM TimeLine WHERE MainId = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn clones_animation_track_with_independent_mixers_and_unknown_columns() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        let summary = writer
            .clone_animation_track_from_template(1, 1, 6, Limits::default())
            .unwrap();
        assert_eq!(summary.template_track_id(), 1);
        assert_eq!(summary.track_id(), 2);
        assert_eq!(summary.timeline_id(), 1);
        assert_eq!(summary.layer_id(), 6);
        assert_eq!(summary.track_uuid()[6] >> 4, 4);
        assert_eq!(summary.track_uuid()[8] >> 6, 2);
        assert_eq!(writer.addition_count(), 2);
        for identifier in [
            summary.primary_mixer_identifier().unwrap(),
            summary.secondary_mixer_identifier().unwrap(),
        ] {
            assert_eq!(identifier.len(), 40);
            assert!(identifier.starts_with(b"extrnlid"));
            assert!(
                identifier[8..]
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'A'..=b'F'))
            );
            assert_ne!(identifier, IDENTIFIER);
            assert_ne!(identifier, SECONDARY_IDENTIFIER);
        }
        assert_ne!(
            summary.primary_mixer_identifier(),
            summary.secondary_mixer_identifier()
        );

        let row = writer
            .database()
            .connection()
            .query_row(
                "SELECT template.TrackNextIndex, clone.TrackNextIndex,
                        clone.BankId, clone.LayerUuidWithTrack, clone.TrackUuid,
                        clone.OpaqueColumn = template.OpaqueColumn,
                        clone._PW_ID != template._PW_ID,
                        clone.TrackActionMixerSize = template.TrackActionMixerSize,
                        clone.TrackActionMixer2Size = template.TrackActionMixer2Size
                 FROM Track AS template JOIN Track AS clone
                 WHERE template.MainId = 1 AND clone.MainId = 2",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, bool>(6)?,
                        row.get::<_, bool>(7)?,
                        row.get::<_, bool>(8)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, 2);
        assert_eq!(row.1, 0);
        assert_eq!(row.2, 2);
        assert_eq!(row.3, vec![0x22; 16]);
        assert_eq!(row.4, summary.track_uuid());
        assert!(row.5);
        assert!(row.6);
        assert!(row.7);
        assert!(row.8);
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT MaxIndex FROM ElemScheme WHERE TableName = 'Track'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );

        assert_eq!(
            writer
                .replace_animation_cel_tag(2, 0, "clone", Limits::default())
                .unwrap(),
            "A"
        );
        let mut output = Vec::new();
        let write_summary = writer.write_to(&mut output).unwrap();
        assert_eq!(write_summary.added_external_bodies(), 2);

        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        rewritten.validate().unwrap();
        let database = rewritten.open_database().unwrap();
        database.quick_check().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        assert_eq!(database.external_chunks().unwrap().len(), 4);
        assert_eq!(
            database
                .connection()
                .query_row(
                    "SELECT count(*) FROM ExternalChunk WHERE typeof(ExternalID) = 'text'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );
        let animation = rewritten
            .read_animation(&database, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(animation.animation_tracks().len(), 2);
        assert_eq!(
            animation.track_for_layer(5).unwrap().keyframes()[0].tag(),
            "A"
        );
        assert_eq!(
            animation.track_for_layer(6).unwrap().keyframes()[0].tag(),
            "clone"
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn rolls_back_cloned_mixers_when_track_insert_fails() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "CREATE UNIQUE INDEX unique_opaque_track_test ON Track(OpaqueColumn)",
                [],
            )
            .unwrap();

        assert!(
            writer
                .clone_animation_track_from_template(1, 1, 6, Limits::default())
                .is_err()
        );
        assert_eq!(writer.addition_count(), 0);
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row("SELECT count(*) FROM Track", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT TrackNextIndex FROM Track WHERE MainId = 1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_track_clone_for_an_already_tracked_layer() {
        let source = writable_animation_sample();
        let mut clip = ClipFile::open(Cursor::new(source)).unwrap();
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_animation_track_from_template(1, 1, 5, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_animation_mixer_aliases_across_tracks_and_columns() {
        fn database(
            target_secondary: &[u8],
            other_primary: Option<&[u8]>,
            other_secondary: Option<&[u8]>,
        ) -> Database {
            let connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE Track (
                        MainId INTEGER,
                        TrackActionMixer BLOB,
                        TrackActionMixer2 BLOB
                     );",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO Track VALUES (1, ?1, ?2)",
                    params![IDENTIFIER, target_secondary],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO Track VALUES (2, ?1, ?2)",
                    params![other_primary, other_secondary],
                )
                .unwrap();
            Database::from_connection(connection).unwrap()
        }

        let clean = database(SECONDARY_IDENTIFIER, None, None);
        let source = writable_animation_track(clean.connection(), clean.schema(), 1).unwrap();
        validate_unique_animation_mixers(clean.connection(), clean.schema(), 1, &source).unwrap();

        for aliased in [
            database(SECONDARY_IDENTIFIER, Some(IDENTIFIER), None),
            database(SECONDARY_IDENTIFIER, Some(SECONDARY_IDENTIFIER), None),
            database(SECONDARY_IDENTIFIER, None, Some(IDENTIFIER)),
        ] {
            let source =
                writable_animation_track(aliased.connection(), aliased.schema(), 1).unwrap();
            assert!(matches!(
                validate_unique_animation_mixers(
                    aliased.connection(),
                    aliased.schema(),
                    1,
                    &source
                ),
                Err(Error::InvalidWrite { .. })
            ));
        }

        let same_row = database(IDENTIFIER, None, None);
        let source = writable_animation_track(same_row.connection(), same_row.schema(), 1).unwrap();
        assert!(matches!(
            validate_unique_animation_mixers(same_row.connection(), same_row.schema(), 1, &source),
            Err(Error::InvalidWrite { .. })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_animation_limits_after_mixer_encoding() {
        let domain_limits = Limits::default().with_max_animation_bytes(1);
        assert!(matches!(
            encode_writable_mixer(&[0], domain_limits),
            Err(Error::LimitExceeded {
                resource: "encoded animation mixer external body",
                limit: 1,
                ..
            })
        ));

        let source = writable_animation_sample();
        let mut clip = ClipFile::open_with_limits(
            Cursor::new(source),
            Limits::default().with_max_write_external_body_size(8),
        )
        .unwrap();
        let writer = clip.writer().unwrap();
        assert!(matches!(
            encode_writable_mixer_for_writer(&writer, &[0], Limits::default()),
            Err(Error::LimitExceeded {
                resource: "encoded animation mixer external body",
                limit: 8,
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_mixers_and_enforces_limits() {
        assert!(matches!(
            parse_image_cel_curve(b"not binc", Limits::default()),
            Err(Error::InvalidAnimation { .. })
        ));

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE TimeLine (
                    MainId INTEGER, BankId INTEGER, FrameRate REAL,
                    StartFrame REAL, EndFrame REAL
                 );
                 INSERT INTO TimeLine VALUES (1, 2, 24, 0, 10);",
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        assert!(matches!(
            database.timelines(Limits::default().with_max_animation_items(0)),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn parses_secondary_double_precision_curves() {
        let curves =
            parse_secondary_animation_curves(&secondary_binc(), Limits::default()).unwrap();
        assert_eq!(curves.len(), 1);
        assert_eq!(curves[0].kind(), "ImageCelName");
        assert_eq!(curves[0].keyframes().len(), 2);
        assert_eq!(curves[0].keyframes()[1].time_60hz(), 60.0);
        assert_eq!(curves[0].keyframes()[1].value(), 1.0);
        assert_eq!(curves[0].keyframes()[0].tag(), Some("A"));
        assert_eq!(curves[0].keyframes()[0].interpolation(), Some("Linear"));
        assert_eq!(curves[0].keyframes()[0].left_slope(), Some(0.0));
        assert_eq!(curves[0].keyframes()[0].revise_constant(), Some(1));
    }

    #[test]
    fn parses_axis_qualified_camera_curves() {
        let primary = parse_animation_curves(&axis_binc(), Limits::default()).unwrap();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].kind(), "ImagePosition");
        assert_eq!(primary[0].axis(), Some("X"));
        assert_eq!(primary[0].keyframes()[0].time_60hz(), 94.0);
        assert_eq!(primary[0].keyframes()[0].value(), 447.0);

        let secondary =
            parse_secondary_animation_curves(&secondary_axis_binc(), Limits::default()).unwrap();
        assert_eq!(secondary.len(), 1);
        assert_eq!(secondary[0].kind(), "ImagePosition");
        assert_eq!(secondary[0].axis(), Some("X"));
        assert_eq!(secondary[0].keyframes()[0].time_60hz(), 94.0);
        assert_eq!(secondary[0].keyframes()[0].value(), 447.0);
    }

    #[test]
    fn reads_and_validates_camera_layer_snapshot() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER, CanvasId INTEGER, LayerType INTEGER,
                    LayerFolder INTEGER, TimeLineLayerKeyFrameEnabled INTEGER,
                    Camera2DResizableImageInfo BLOB,
                    Camera2DOriginalFrameCenterX REAL,
                    Camera2DOriginalFrameCenterY REAL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO Layer VALUES (7, 1, 512, 1, 1, ?1, 432, 324)",
                params![camera_transform()],
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        let camera = database
            .camera_2d_layer(7, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(camera.layer_id(), 7);
        assert_eq!(camera.canvas_id(), 1);
        assert!(camera.keyframes_enabled());
        assert_eq!(
            camera.original_frame_center(),
            Camera2DPoint { x: 432.0, y: 324.0 }
        );
        assert_eq!(camera.transform().width(), 720);
        assert_eq!(camera.transform().height(), 540);
        assert_eq!(camera.transform().header_size(), 120);
        assert_eq!(camera.transform().point_record_size(), 16);
        assert_eq!(
            camera.transform().position(),
            Camera2DPoint { x: 447.0, y: 327.0 }
        );
        assert_eq!(camera.transform().corners().len(), 4);
        assert_eq!(camera.transform().raw(), camera_transform());
        assert!(matches!(
            database.camera_2d_layer(7, Limits::default().with_max_animation_items(3)),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn parses_typed_and_unknown_track_values() {
        let mut bytes = Vec::new();
        push_be_u32(&mut bytes, 8);
        push_be_u32(&mut bytes, 4);
        push_value_record(
            &mut bytes,
            "PlayTime",
            "",
            0,
            &2.5_f64.to_bits().to_be_bytes(),
        );
        push_value_record(&mut bytes, "ImageCelName", "A", 2, &7_u32.to_be_bytes());
        let mut vector = Vec::new();
        vector.extend_from_slice(&447.0_f64.to_be_bytes());
        vector.extend_from_slice(&327.0_f64.to_be_bytes());
        push_value_record(&mut bytes, "ImagePosition", "", 3, &vector);
        push_value_record(&mut bytes, "FutureValue", "opaque", 99, &[1, 2, 3]);

        let entries = parse_track_value_map(&bytes, Limits::default()).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].value(), &AnimationTrackValue::Float(2.5));
        assert_eq!(
            entries[1].value(),
            &AnimationTrackValue::IndexedText {
                text: "A".to_owned(),
                numeric_value: 7,
            }
        );
        assert_eq!(
            entries[2].value(),
            &AnimationTrackValue::Vector2 { x: 447.0, y: 327.0 }
        );
        assert_eq!(
            entries[3].value(),
            &AnimationTrackValue::Unknown {
                kind: 99,
                text: "opaque".to_owned(),
                payload: Box::from([1, 2, 3]),
            }
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn rebuilds_typed_values_and_patches_only_known_curve_fields() {
        let mut values = parse_track_value_map(&track_value_map(), Limits::default()).unwrap();
        values[0].value = AnimationTrackValue::IndexedText {
            text: "C".to_owned(),
            numeric_value: 2,
        };
        let encoded = encode_track_value_map(&values, Limits::default()).unwrap();
        assert_eq!(
            parse_track_value_map(&encoded, Limits::default()).unwrap(),
            values
        );

        let mut primary = binc();
        let original = patch_primary_curve_numeric(
            &mut primary,
            "ImageCelName",
            None,
            1,
            120.0,
            2.0,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(original.time_60hz(), 60.0);
        assert_eq!(original.value(), 1.0);
        assert_eq!(
            patch_primary_curve_tag(&mut primary, 0, "new cel", Limits::default()).unwrap(),
            "A"
        );
        let curve = parse_animation_curves(&primary, Limits::default()).unwrap();
        assert_eq!(curve[0].keyframes()[0].tag(), Some("new cel"));
        assert_eq!(curve[0].keyframes()[1].time_60hz(), 120.0);
        assert_eq!(curve[0].keyframes()[1].value(), 2.0);
        assert_eq!(curve[0].keyframes()[1].interpolation(), Some("Linear"));

        let mut secondary = secondary_binc();
        patch_secondary_curve_numeric(
            &mut secondary,
            "ImageCelName",
            None,
            1,
            120.0,
            2.0,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            patch_secondary_curve_tag(&mut secondary, 0, "new cel", Limits::default()).unwrap(),
            Some("A".to_owned())
        );
        let curve = parse_secondary_animation_curves(&secondary, Limits::default()).unwrap();
        assert_eq!(curve[0].keyframes()[0].tag(), Some("new cel"));
        assert_eq!(curve[0].keyframes()[1].time_60hz(), 120.0);
        assert_eq!(curve[0].keyframes()[1].value(), 2.0);
        assert_eq!(curve[0].keyframes()[1].interpolation(), Some("Linear"));
    }

    #[test]
    fn validates_typed_camera_track_values() {
        let entry = |name: &str, value: AnimationTrackValue| AnimationTrackValueEntry {
            name: name.to_owned(),
            value,
        };
        let values = [
            entry(
                "ImageCenter",
                AnimationTrackValue::Vector2 { x: 360.0, y: 270.0 },
            ),
            entry(
                "ImagePosition",
                AnimationTrackValue::Vector2 { x: 447.0, y: 327.0 },
            ),
            entry("ImageRotation", AnimationTrackValue::Float(0.0)),
            entry("ImageScale", AnimationTrackValue::Float(100.0)),
            entry("Opacity", AnimationTrackValue::Float(100.0)),
        ];
        let camera = parse_camera_2d_track_values(&values).unwrap();
        assert_eq!(
            camera.image_position(),
            Camera2DPoint { x: 447.0, y: 327.0 }
        );
        assert_eq!(camera.rotation(), 0.0);
        assert_eq!(camera.scale(), 100.0);
        assert_eq!(camera.opacity(), 100.0);
    }

    #[test]
    fn rejects_malformed_track_value_maps() {
        assert!(matches!(
            parse_track_value_map(&[0; 8], Limits::default()),
            Err(Error::InvalidAnimation { .. })
        ));

        let mut truncated = track_value_map();
        truncated.pop();
        assert!(matches!(
            parse_track_value_map(&truncated, Limits::default()),
            Err(Error::InvalidAnimation { .. })
        ));

        assert!(matches!(
            parse_track_value_map(
                &track_value_map(),
                Limits::default().with_max_animation_items(0)
            ),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn validates_the_timeline_track_chain() {
        let source = |id, next_track_id| AnimationTrackSource {
            id,
            kind: 1000,
            next_track_id,
            external_identifier: None,
            secondary_external_identifier: None,
            value_map: None,
            layer_uuid: None,
        };
        let valid = [source(1, Some(2)), source(2, None)];
        validate_track_chain(&valid, 1).unwrap();

        let cycle = [source(1, Some(2)), source(2, Some(1))];
        assert!(matches!(
            validate_track_chain(&cycle, 1),
            Err(Error::InvalidAnimation { .. })
        ));
        assert!(matches!(
            validate_track_chain(&valid, 2),
            Err(Error::InvalidAnimation { .. })
        ));
    }

    #[test]
    fn classifies_verified_track_kinds() {
        assert!(AnimationTrackKind::new(1000).is_folder());
        assert_eq!(AnimationTrackKind::new(1000).known_name(), Some("folder"));
        assert!(AnimationTrackKind::new(2000).is_image_cel());
        assert!(AnimationTrackKind::new(2001).is_static_image());
        assert!(AnimationTrackKind::new(2003).is_paper());
        assert!(AnimationTrackKind::new(2005).is_camera_2d());
        assert!(AnimationTrackKind::new(4000).is_play_time());
        assert!(AnimationTrackKind::new(4001).is_audio());
        assert!(!AnimationTrackKind::new(9999).is_static_image());
        assert_eq!(AnimationTrackKind::new(9999).known_name(), None);
    }
}
