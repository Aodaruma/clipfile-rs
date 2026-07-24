use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek},
};

use rusqlite::{OptionalExtension, Row, params};

use crate::{ClipFile, Database, Error, Limits, Result};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Document-level settings from the single `Project` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    internal_version: String,
    name: Option<String>,
    primary_canvas_id: Option<i64>,
}

impl Project {
    /// Internal document format version reported by CLIP STUDIO PAINT.
    #[must_use]
    pub fn internal_version(&self) -> &str {
        &self.internal_version
    }

    /// Project name when stored in the file.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Primary canvas reference when present and nonzero.
    #[must_use]
    pub const fn primary_canvas_id(&self) -> Option<i64> {
        self.primary_canvas_id
    }
}

/// One canvas and its root/current-layer references.
#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    id: i64,
    unit: i64,
    width: f64,
    height: f64,
    resolution: f64,
    root_layer_id: i64,
    current_layer_id: Option<i64>,
}

impl Canvas {
    /// Stable SQLite `MainId` within the document.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Opaque `CanvasUnit` value. Zero is observed for pixels.
    #[must_use]
    pub const fn unit(&self) -> i64 {
        self.unit
    }

    /// Canvas width as stored by SQLite.
    #[must_use]
    pub const fn width(&self) -> f64 {
        self.width
    }

    /// Canvas height as stored by SQLite.
    #[must_use]
    pub const fn height(&self) -> f64 {
        self.height
    }

    /// Canvas resolution.
    #[must_use]
    pub const fn resolution(&self) -> f64 {
        self.resolution
    }

    /// Special root-folder layer for this canvas.
    #[must_use]
    pub const fn root_layer_id(&self) -> i64 {
        self.root_layer_id
    }

    /// Current layer when present and nonzero.
    #[must_use]
    pub const fn current_layer_id(&self) -> Option<i64> {
        self.current_layer_id
    }
}

/// Encoded preview image stored in the `CanvasPreview` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasPreview {
    id: i64,
    canvas_id: i64,
    image_type: i64,
    width: u32,
    height: u32,
    data: Box<[u8]>,
}

impl CanvasPreview {
    /// Stable SQLite `MainId` within the document.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Canvas to which this preview belongs.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Original, forward-compatible `ImageType` value.
    #[must_use]
    pub const fn image_type(&self) -> i64 {
        self.image_type
    }

    /// Preview width declared by SQLite and cross-checked with the PNG header.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Preview height declared by SQLite and cross-checked with the PNG header.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Encoded image bytes. Observed CLIP files store PNG data.
    #[must_use]
    pub const fn data(&self) -> &[u8] {
        &self.data
    }

    /// Whether the encoded bytes begin with the PNG signature.
    #[must_use]
    pub fn is_png(&self) -> bool {
        self.data.starts_with(PNG_SIGNATURE)
    }

    /// Takes ownership of the encoded image bytes.
    #[must_use]
    pub fn into_data(self) -> Box<[u8]> {
        self.data
    }
}

impl Database {
    /// Loads the encoded preview for one canvas under the configured size limit.
    pub fn canvas_preview(&self, canvas_id: i64, limits: Limits) -> Result<Option<CanvasPreview>> {
        for column in [
            "MainId",
            "CanvasId",
            "ImageType",
            "ImageWidth",
            "ImageHeight",
            "ImageData",
        ] {
            self.require_column("CanvasPreview", column)?;
        }
        let raw = self
            .connection()
            .query_row(
                "SELECT MainId, CanvasId, ImageType, ImageWidth, ImageHeight, ImageData \
                 FROM CanvasPreview WHERE CanvasId = ?1 ORDER BY MainId LIMIT 1",
                params![canvas_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, canvas_id, image_type, width, height, data)) = raw else {
            return Ok(None);
        };
        let width = validate_preview_dimension("width", width, limits.max_canvas_dimension())?;
        let height = validate_preview_dimension("height", height, limits.max_canvas_dimension())?;
        let data_size = u64::try_from(data.len()).map_err(|_| Error::LimitExceeded {
            resource: "canvas preview bytes",
            value: u64::MAX,
            limit: limits.max_preview_bytes(),
        })?;
        if data_size > limits.max_preview_bytes() {
            return Err(Error::LimitExceeded {
                resource: "canvas preview bytes",
                value: data_size,
                limit: limits.max_preview_bytes(),
            });
        }
        validate_png_preview(&data, width, height)?;
        Ok(Some(CanvasPreview {
            id,
            canvas_id,
            image_type,
            width,
            height,
            data: data.into_boxed_slice(),
        }))
    }
}

/// Raw, forward-compatible `LayerType` flags.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LayerKind(i64);

impl LayerKind {
    /// Original SQLite value.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Whether the observed pixel-layer bit is set.
    #[must_use]
    pub const fn is_pixel(self) -> bool {
        self.0 & 1 != 0
    }

    /// Whether the observed root-folder bit is set.
    #[must_use]
    pub const fn is_root_folder(self) -> bool {
        self.0 & 256 != 0
    }

    /// Whether the observed 2D-camera layer bit is set.
    #[must_use]
    pub const fn is_camera_2d(self) -> bool {
        self.0 & 512 != 0
    }

    /// Whether the observed correction-layer bit is set.
    #[must_use]
    pub const fn is_correction(self) -> bool {
        self.0 & 4_096 != 0
    }
}

/// Raw, forward-compatible `LayerComposite` value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlendMode(i64);

impl BlendMode {
    /// Normal composition.
    pub const NORMAL: Self = Self(0);
    /// Multiply composition.
    pub const MULTIPLY: Self = Self(2);
    /// Screen composition.
    pub const SCREEN: Self = Self(8);
    /// Overlay composition.
    pub const OVERLAY: Self = Self(14);

    /// Original SQLite value.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }
}

/// Core, version-stable fields from one `Layer` row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layer {
    id: i64,
    canvas_id: i64,
    name: Option<String>,
    kind: LayerKind,
    opacity: i64,
    blend_mode: BlendMode,
    lock_flags: i64,
    clip_flags: i64,
    masking_flags: i64,
    folder_flags: i64,
    visibility_flags: i64,
    next_sibling_id: Option<i64>,
    first_child_id: Option<i64>,
    render_mipmap_id: Option<i64>,
    mask_mipmap_id: Option<i64>,
}

impl Layer {
    /// Stable SQLite `MainId` within the document.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Layer name when stored as text.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Raw layer-type flags with conservative helpers.
    #[must_use]
    pub const fn kind(&self) -> LayerKind {
        self.kind
    }

    /// Raw opacity, observed in the inclusive range 0–256.
    #[must_use]
    pub const fn opacity(&self) -> i64 {
        self.opacity
    }

    /// Opacity normalized to 0.0–1.0.
    #[must_use]
    pub fn opacity_fraction(&self) -> f64 {
        self.opacity as f64 / 256.0
    }

    /// Raw blend-mode value with common constants.
    #[must_use]
    pub const fn blend_mode(&self) -> BlendMode {
        self.blend_mode
    }

    /// Original `LayerLock` flags.
    #[must_use]
    pub const fn lock_flags(&self) -> i64 {
        self.lock_flags
    }

    /// Original `LayerClip` flags.
    #[must_use]
    pub const fn clip_flags(&self) -> i64 {
        self.clip_flags
    }

    /// Original `LayerMasking` flags.
    #[must_use]
    pub const fn masking_flags(&self) -> i64 {
        self.masking_flags
    }

    /// Original `LayerFolder` flags.
    #[must_use]
    pub const fn folder_flags(&self) -> i64 {
        self.folder_flags
    }

    /// Original `LayerVisibility` flags.
    #[must_use]
    pub const fn visibility_flags(&self) -> i64 {
        self.visibility_flags
    }

    /// Whether the observed visible bit is set.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.visibility_flags & 1 != 0
    }

    /// Whether the observed folder bit is set.
    #[must_use]
    pub const fn is_folder(&self) -> bool {
        self.folder_flags & 1 != 0
    }

    /// Whether the observed closed-folder bit is set.
    #[must_use]
    pub const fn is_folder_closed(&self) -> bool {
        self.folder_flags & 16 != 0
    }

    /// Whether clipping to the layer below is enabled.
    #[must_use]
    pub const fn is_clipped(&self) -> bool {
        self.clip_flags != 0
    }

    /// Next sibling in the on-disk linked list.
    #[must_use]
    pub const fn next_sibling_id(&self) -> Option<i64> {
        self.next_sibling_id
    }

    /// First child in the on-disk linked list.
    #[must_use]
    pub const fn first_child_id(&self) -> Option<i64> {
        self.first_child_id
    }

    /// Base render mipmap reference, if present and nonzero.
    #[must_use]
    pub const fn render_mipmap_id(&self) -> Option<i64> {
        self.render_mipmap_id
    }

    /// Layer-mask mipmap reference, if present and nonzero.
    #[must_use]
    pub const fn mask_mipmap_id(&self) -> Option<i64> {
        self.mask_mipmap_id
    }
}

/// Validated layer hierarchy for one canvas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerTree {
    canvas_id: i64,
    root_layer_id: i64,
    children: BTreeMap<i64, Vec<i64>>,
    unreachable_layer_ids: Vec<i64>,
}

impl LayerTree {
    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Special root layer.
    #[must_use]
    pub const fn root_layer_id(&self) -> i64 {
        self.root_layer_id
    }

    /// Ordered children for a reachable layer, or `None` for an unknown ID.
    #[must_use]
    pub fn children_of(&self, layer_id: i64) -> Option<&[i64]> {
        self.children.get(&layer_id).map(Vec::as_slice)
    }

    /// Ordered top-level layers below the special root.
    #[must_use]
    pub fn root_children(&self) -> &[i64] {
        self.children_of(self.root_layer_id).unwrap_or_default()
    }

    /// Number of layers reachable from the root, including the root itself.
    #[must_use]
    pub fn reachable_layer_count(&self) -> usize {
        self.children.len()
    }

    /// Rows belonging to the canvas but not reachable from its current root.
    #[must_use]
    pub fn unreachable_layer_ids(&self) -> &[i64] {
        &self.unreachable_layer_ids
    }
}

/// Read-only high-level view of the project, canvases, layers, and trees.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    project: Project,
    canvases: Vec<Canvas>,
    layers: Vec<Layer>,
    trees: Vec<LayerTree>,
}

impl Document {
    /// Loads and validates the model using caller-provided safety limits.
    pub fn load(database: &Database, limits: Limits) -> Result<Self> {
        let project = read_project(database)?;
        let canvases = read_canvases(database, limits)?;
        let layers = read_layers(database, limits)?;
        validate_unique_ids(&canvases, &layers)?;
        let trees = canvases
            .iter()
            .map(|canvas| build_tree(canvas, &layers, limits.max_layer_tree_depth()))
            .collect::<Result<Vec<_>>>()?;
        if let Some(primary) = project.primary_canvas_id {
            if !canvases.iter().any(|canvas| canvas.id == primary) {
                return invalid_document(format!(
                    "ProjectCanvas {primary} does not refer to a Canvas row"
                ));
            }
        }
        Ok(Self {
            project,
            canvases,
            layers,
            trees,
        })
    }

    /// Project settings.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    /// All canvas rows in SQLite order.
    #[must_use]
    pub fn canvases(&self) -> &[Canvas] {
        &self.canvases
    }

    /// All layer rows in SQLite order, including unreachable rows.
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Looks up a layer by `MainId`.
    #[must_use]
    pub fn layer(&self, id: i64) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.id == id)
    }

    /// Looks up a canvas by `MainId`.
    #[must_use]
    pub fn canvas(&self, id: i64) -> Option<&Canvas> {
        self.canvases.iter().find(|canvas| canvas.id == id)
    }

    /// Validated layer trees, one per canvas.
    #[must_use]
    pub fn layer_trees(&self) -> &[LayerTree] {
        &self.trees
    }

    /// Looks up a layer tree by canvas ID.
    #[must_use]
    pub fn layer_tree(&self, canvas_id: i64) -> Option<&LayerTree> {
        self.trees.iter().find(|tree| tree.canvas_id == canvas_id)
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Loads the embedded database and builds the high-level document model.
    pub fn read_document(&mut self) -> Result<Document> {
        let limits = self.limits();
        let database = self.open_database()?;
        Document::load(&database, limits)
    }
}

fn read_project(database: &Database) -> Result<Project> {
    database.require_column("Project", "ProjectInternalVersion")?;
    database.require_column("Project", "ProjectCanvas")?;
    let name_expression = if database.schema().has_column("Project", "ProjectName") {
        "ProjectName"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT ProjectInternalVersion, {name_expression}, ProjectCanvas FROM Project ORDER BY rowid LIMIT 1"
    );
    database
        .connection()
        .query_row(&sql, [], |row| {
            Ok(Project {
                internal_version: row.get(0)?,
                name: row.get(1)?,
                primary_canvas_id: normalize_id(row.get(2)?),
            })
        })
        .optional()?
        .ok_or_else(|| Error::InvalidDocument {
            reason: "Project table is empty".to_owned(),
        })
}

fn read_canvases(database: &Database, limits: Limits) -> Result<Vec<Canvas>> {
    for column in [
        "MainId",
        "CanvasUnit",
        "CanvasWidth",
        "CanvasHeight",
        "CanvasResolution",
        "CanvasRootFolder",
        "CanvasCurrentLayer",
    ] {
        database.require_column("Canvas", column)?;
    }
    let mut statement = database.connection().prepare(
        "SELECT MainId, CanvasUnit, CanvasWidth, CanvasHeight, CanvasResolution, \
         CanvasRootFolder, CanvasCurrentLayer FROM Canvas ORDER BY rowid",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(Canvas {
                id: row.get(0)?,
                unit: row.get(1)?,
                width: row.get(2)?,
                height: row.get(3)?,
                resolution: row.get(4)?,
                root_layer_id: row.get(5)?,
                current_layer_id: normalize_id(row.get(6)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for canvas in &rows {
        validate_canvas_dimension("width", canvas.width, limits.max_canvas_dimension())?;
        validate_canvas_dimension("height", canvas.height, limits.max_canvas_dimension())?;
        if !canvas.resolution.is_finite() || canvas.resolution <= 0.0 {
            return invalid_document(format!(
                "Canvas {} has invalid resolution {}",
                canvas.id, canvas.resolution
            ));
        }
    }
    Ok(rows)
}

fn read_layers(database: &Database, limits: Limits) -> Result<Vec<Layer>> {
    const COLUMNS: [&str; 15] = [
        "MainId",
        "CanvasId",
        "LayerName",
        "LayerType",
        "LayerOpacity",
        "LayerComposite",
        "LayerLock",
        "LayerClip",
        "LayerMasking",
        "LayerFolder",
        "LayerVisibility",
        "LayerNextIndex",
        "LayerFirstChildIndex",
        "LayerRenderMipmap",
        "LayerLayerMaskMipmap",
    ];
    for column in COLUMNS {
        database.require_column("Layer", column)?;
    }
    let count = database.row_count("Layer")?;
    if count > limits.max_layers() {
        return Err(Error::LimitExceeded {
            resource: "layer count",
            value: count,
            limit: limits.max_layers(),
        });
    }
    let mut statement = database.connection().prepare(
        "SELECT MainId, CanvasId, LayerName, LayerType, LayerOpacity, LayerComposite, \
         LayerLock, LayerClip, LayerMasking, LayerFolder, LayerVisibility, LayerNextIndex, \
         LayerFirstChildIndex, LayerRenderMipmap, LayerLayerMaskMipmap \
         FROM Layer ORDER BY rowid",
    )?;
    let layers = statement
        .query_map([], read_layer)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for layer in &layers {
        if !(0..=256).contains(&layer.opacity) {
            return invalid_document(format!(
                "Layer {} has opacity {}, expected 0..=256",
                layer.id, layer.opacity
            ));
        }
    }
    Ok(layers)
}

fn read_layer(row: &Row<'_>) -> rusqlite::Result<Layer> {
    Ok(Layer {
        id: row.get(0)?,
        canvas_id: row.get(1)?,
        name: row.get(2)?,
        kind: LayerKind(row.get(3)?),
        opacity: row.get(4)?,
        blend_mode: BlendMode(row.get(5)?),
        lock_flags: row.get(6)?,
        clip_flags: row.get(7)?,
        masking_flags: row.get(8)?,
        folder_flags: row.get(9)?,
        visibility_flags: row.get(10)?,
        next_sibling_id: normalize_id(row.get(11)?),
        first_child_id: normalize_id(row.get(12)?),
        render_mipmap_id: normalize_id(row.get(13)?),
        mask_mipmap_id: normalize_id(row.get(14)?),
    })
}

fn validate_unique_ids(canvases: &[Canvas], layers: &[Layer]) -> Result<()> {
    let mut canvas_ids = BTreeSet::new();
    for canvas in canvases {
        if canvas.id <= 0 || !canvas_ids.insert(canvas.id) {
            return invalid_document(format!("invalid or duplicate Canvas MainId {}", canvas.id));
        }
    }
    let mut layer_ids = BTreeSet::new();
    for layer in layers {
        if layer.id <= 0 || !layer_ids.insert(layer.id) {
            return invalid_document(format!("invalid or duplicate Layer MainId {}", layer.id));
        }
        if !canvas_ids.contains(&layer.canvas_id) {
            return invalid_document(format!(
                "Layer {} refers to missing Canvas {}",
                layer.id, layer.canvas_id
            ));
        }
    }
    Ok(())
}

fn build_tree(canvas: &Canvas, layers: &[Layer], depth_limit: u64) -> Result<LayerTree> {
    let by_id = layers
        .iter()
        .filter(|layer| layer.canvas_id == canvas.id)
        .map(|layer| (layer.id, layer))
        .collect::<BTreeMap<_, _>>();
    if !by_id.contains_key(&canvas.root_layer_id) {
        return invalid_document(format!(
            "Canvas {} root layer {} is missing",
            canvas.id, canvas.root_layer_id
        ));
    }
    if let Some(current) = canvas.current_layer_id {
        if !by_id.contains_key(&current) {
            return invalid_document(format!(
                "Canvas {} current layer {current} is missing",
                canvas.id
            ));
        }
    }

    let mut children = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let mut stack = vec![(canvas.root_layer_id, 0_u64)];
    parents.insert(canvas.root_layer_id, None);
    while let Some((parent_id, depth)) = stack.pop() {
        if depth > depth_limit {
            return Err(Error::LimitExceeded {
                resource: "layer tree depth",
                value: depth,
                limit: depth_limit,
            });
        }
        let parent = by_id[&parent_id];
        let mut ordered = Vec::new();
        let mut sibling_seen = BTreeSet::new();
        let mut next = parent.first_child_id;
        while let Some(child_id) = next {
            if !sibling_seen.insert(child_id) {
                return invalid_document(format!(
                    "sibling cycle below Layer {parent_id} at Layer {child_id}"
                ));
            }
            let child = by_id.get(&child_id).ok_or_else(|| Error::InvalidDocument {
                reason: format!(
                    "Layer {parent_id} refers to missing child Layer {child_id} on Canvas {}",
                    canvas.id
                ),
            })?;
            if parents.insert(child_id, Some(parent_id)).is_some() {
                return invalid_document(format!(
                    "Layer {child_id} is reachable more than once or forms a cycle"
                ));
            }
            ordered.push(child_id);
            stack.push((child_id, depth.saturating_add(1)));
            next = child.next_sibling_id;
        }
        children.insert(parent_id, ordered);
    }
    let unreachable_layer_ids = by_id
        .keys()
        .filter(|id| !parents.contains_key(id))
        .copied()
        .collect();
    Ok(LayerTree {
        canvas_id: canvas.id,
        root_layer_id: canvas.root_layer_id,
        children,
        unreachable_layer_ids,
    })
}

fn validate_canvas_dimension(name: &str, value: f64, limit: u32) -> Result<()> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(limit) {
        return invalid_document(format!(
            "Canvas {name} {value} is outside the supported range 0..={limit}"
        ));
    }
    Ok(())
}

fn validate_preview_dimension(name: &str, value: i64, limit: u32) -> Result<u32> {
    let value = u32::try_from(value).map_err(|_| Error::InvalidDocument {
        reason: format!("CanvasPreview {name} {value} is not a positive 32-bit value"),
    })?;
    if value == 0 || value > limit {
        return invalid_document(format!(
            "CanvasPreview {name} {value} is outside the supported range 1..={limit}"
        ));
    }
    Ok(value)
}

fn validate_png_preview(data: &[u8], width: u32, height: u32) -> Result<()> {
    if !data.starts_with(PNG_SIGNATURE) {
        return Ok(());
    }
    if data.len() < 24 {
        return invalid_document("CanvasPreview PNG header is truncated".to_owned());
    }
    let ihdr_size = u32::from_be_bytes(data[8..12].try_into().unwrap());
    if ihdr_size != 13 || &data[12..16] != b"IHDR" {
        return invalid_document("CanvasPreview PNG does not begin with IHDR".to_owned());
    }
    let png_width = u32::from_be_bytes(data[16..20].try_into().unwrap());
    let png_height = u32::from_be_bytes(data[20..24].try_into().unwrap());
    if (png_width, png_height) != (width, height) {
        return invalid_document(format!(
            "CanvasPreview dimensions {width}x{height} do not match PNG IHDR {png_width}x{png_height}"
        ));
    }
    Ok(())
}

fn normalize_id(value: Option<i64>) -> Option<i64> {
    value.filter(|value| *value != 0)
}

fn invalid_document<T>(reason: String) -> Result<T> {
    Err(Error::InvalidDocument { reason })
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn database() -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Project (
                    ProjectInternalVersion TEXT, ProjectName TEXT, ProjectCanvas INTEGER
                 );
                 INSERT INTO Project VALUES ('1.1.0', 'test', 10);
                 CREATE TABLE Canvas (
                    MainId INTEGER, CanvasUnit INTEGER, CanvasWidth REAL, CanvasHeight REAL,
                    CanvasResolution REAL, CanvasRootFolder INTEGER, CanvasCurrentLayer INTEGER
                 );
                 INSERT INTO Canvas VALUES (10, 0, 640, 480, 72, 1, 4);
                 CREATE TABLE Layer (
                    MainId INTEGER, CanvasId INTEGER, LayerName TEXT, LayerType INTEGER,
                    LayerOpacity INTEGER, LayerComposite INTEGER, LayerLock INTEGER,
                    LayerClip INTEGER, LayerMasking INTEGER, LayerFolder INTEGER,
                    LayerVisibility INTEGER, LayerNextIndex INTEGER,
                    LayerFirstChildIndex INTEGER, LayerRenderMipmap INTEGER,
                    LayerLayerMaskMipmap INTEGER
                 );
                 INSERT INTO Layer VALUES (1,10,'',256,256,0,0,0,0,1,1,0,2,0,0);
                 INSERT INTO Layer VALUES (2,10,'paint',1,128,2,0,0,0,0,1,3,0,20,0);
                 INSERT INTO Layer VALUES (3,10,'folder',0,256,0,0,0,0,17,1,0,4,0,0);
                 INSERT INTO Layer VALUES (4,10,'child',1,256,0,0,0,0,0,0,0,0,21,22);
                 INSERT INTO Layer VALUES (5,10,'stale',1,256,0,0,0,0,0,1,0,0,0,0);
                 CREATE TABLE CanvasPreview (
                    MainId INTEGER, CanvasId INTEGER, ImageType INTEGER,
                    ImageWidth INTEGER, ImageHeight INTEGER, ImageData BLOB
                 );
                 INSERT INTO CanvasPreview VALUES (
                    30, 10, 1, 640, 480,
                    X'89504E470D0A1A0A0000000D4948445200000280000001E0'
                 );",
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[test]
    fn builds_layer_tree_and_preserves_unreachable_rows() {
        let document = Document::load(&database(), Limits::default()).unwrap();
        assert_eq!(document.project().internal_version(), "1.1.0");
        assert_eq!(document.canvases()[0].width(), 640.0);
        let tree = document.layer_tree(10).unwrap();
        assert_eq!(tree.root_children(), &[2, 3]);
        assert_eq!(tree.children_of(3), Some(&[4][..]));
        assert_eq!(tree.reachable_layer_count(), 4);
        assert_eq!(tree.unreachable_layer_ids(), &[5]);
        let paint = document.layer(2).unwrap();
        assert_eq!(paint.blend_mode(), BlendMode::MULTIPLY);
        assert_eq!(paint.opacity_fraction(), 0.5);
    }

    #[test]
    fn detects_sibling_cycles() {
        let database = database();
        database
            .connection()
            .execute("UPDATE Layer SET LayerNextIndex = 4 WHERE MainId = 4", [])
            .unwrap();
        assert!(matches!(
            Document::load(&database, Limits::default()),
            Err(Error::InvalidDocument { .. })
        ));
    }

    #[test]
    fn reads_and_validates_canvas_preview_png() {
        let preview = database()
            .canvas_preview(10, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(preview.id(), 30);
        assert_eq!(preview.canvas_id(), 10);
        assert_eq!(preview.image_type(), 1);
        assert_eq!((preview.width(), preview.height()), (640, 480));
        assert!(preview.is_png());
        assert_eq!(preview.data().len(), 24);
        assert!(
            database()
                .canvas_preview(999, Limits::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_invalid_or_oversized_canvas_preview() {
        assert!(matches!(
            database().canvas_preview(10, Limits::default().with_max_preview_bytes(8)),
            Err(Error::LimitExceeded {
                resource: "canvas preview bytes",
                ..
            })
        ));
        let database = database();
        database
            .connection()
            .execute(
                "UPDATE CanvasPreview SET ImageWidth = 320 WHERE CanvasId = 10",
                [],
            )
            .unwrap();
        assert!(matches!(
            database.canvas_preview(10, Limits::default()),
            Err(Error::InvalidDocument { .. })
        ));
    }

    #[test]
    fn enforces_layer_limit() {
        assert!(matches!(
            Document::load(&database(), Limits::default().with_max_layers(4)),
            Err(Error::LimitExceeded {
                resource: "layer count",
                ..
            })
        ));
    }
}
