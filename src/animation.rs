use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    str,
};

use flate2::read::ZlibDecoder;
use rusqlite::{OptionalExtension, params, types::ValueRef};

use crate::{ByteOrder, ClipFile, Database, Error, ExternalBody, Limits, Result};

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
        Ok(Some(Animation {
            timeline,
            tracks,
            animation_tracks,
        }))
    }
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
    if database.schema().has_column("Track", "TrackNextIndex")
        && let Some(first_track_id) = timeline.first_track_id
    {
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

    use super::*;

    const IDENTIFIER: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";
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
        assert!(AnimationTrackKind::new(2000).is_image_cel());
        assert!(AnimationTrackKind::new(2001).is_static_image());
        assert!(AnimationTrackKind::new(2003).is_paper());
        assert!(AnimationTrackKind::new(2005).is_camera_2d());
        assert!(AnimationTrackKind::new(4000).is_play_time());
        assert!(AnimationTrackKind::new(4001).is_audio());
        assert!(!AnimationTrackKind::new(9999).is_static_image());
    }
}
