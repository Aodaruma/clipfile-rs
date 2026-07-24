use std::collections::BTreeSet;

use rusqlite::{params, types::ValueRef};

use crate::{Database, Error, Limits, Result};

const MANAGER_TABLE: &str = "SpecialRulerManager";
const MANAGER_COLUMNS: [(&str, &str, RulerKind); 9] = [
    ("FirstParallel", "RulerParallel", RulerKind::Parallel),
    (
        "FirstCurveParallel",
        "RulerCurveParallel",
        RulerKind::CurveParallel,
    ),
    ("FirstMultiCurve", "RulerMultiCurve", RulerKind::MultiCurve),
    ("FirstEmit", "RulerEmit", RulerKind::Emit),
    ("FirstCurveEmit", "RulerCurveEmit", RulerKind::CurveEmit),
    (
        "FirstConcentricCircle",
        "RulerConcentricCircle",
        RulerKind::ConcentricCircle,
    ),
    ("FirstGuide", "RulerGuide", RulerKind::Guide),
    (
        "FirstPerspective",
        "RulerPerspective",
        RulerKind::Perspective,
    ),
    ("FirstSymmetry", "RulerSymmetry", RulerKind::Symmetry),
];

/// Known special-ruler table kind.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum RulerKind {
    /// Parallel line.
    Parallel,
    /// Parallel curve.
    CurveParallel,
    /// Multiple curve.
    MultiCurve,
    /// Radial line (`RulerEmit`).
    Emit,
    /// Radial curve (`RulerCurveEmit`).
    CurveEmit,
    /// Concentric circle.
    ConcentricCircle,
    /// Horizontal or vertical guide.
    Guide,
    /// Perspective ruler.
    Perspective,
    /// Symmetry ruler.
    Symmetry,
}

/// One finite two-dimensional ruler coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerPoint {
    x: f64,
    y: f64,
}

impl RulerPoint {
    /// X coordinate.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Y coordinate.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// One point from a curve ruler's big-endian `PointData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerCurvePoint {
    position: RulerPoint,
    thickness: i32,
}

impl RulerCurvePoint {
    /// Stored point position.
    #[must_use]
    pub const fn position(self) -> RulerPoint {
        self.position
    }

    /// Raw point thickness.
    #[must_use]
    pub const fn thickness(self) -> i32 {
        self.thickness
    }
}

/// Validated curve-ruler point payload with unknown header words preserved.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerCurveData {
    header_size: u32,
    metadata: [i32; 4],
    points: Vec<RulerCurvePoint>,
    raw: Box<[u8]>,
}

impl RulerCurveData {
    /// Declared header size.
    #[must_use]
    pub const fn header_size(&self) -> u32 {
        self.header_size
    }

    /// Four observed but not semantically named header words.
    #[must_use]
    pub const fn metadata(&self) -> [i32; 4] {
        self.metadata
    }

    /// Ordered curve points.
    #[must_use]
    pub fn points(&self) -> &[RulerCurvePoint] {
        &self.points
    }

    /// Original bounded `PointData` bytes.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// One validated perspective-ruler vanishing-point row.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerVanishPoint {
    id: i64,
    flags: i64,
    position: RulerPoint,
    parallel_angle: f64,
    guide_count: u64,
    guide_record_size: u64,
    guide_data: Box<[u8]>,
}

impl RulerVanishPoint {
    /// SQLite `MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Raw `Flag`.
    #[must_use]
    pub const fn flags(&self) -> i64 {
        self.flags
    }

    /// Vanishing-point coordinate.
    #[must_use]
    pub const fn position(&self) -> RulerPoint {
        self.position
    }

    /// Stored parallel angle.
    #[must_use]
    pub const fn parallel_angle(&self) -> f64 {
        self.parallel_angle
    }

    /// Declared guide record count.
    #[must_use]
    pub const fn guide_count(&self) -> u64 {
        self.guide_count
    }

    /// Declared bytes per guide record.
    #[must_use]
    pub const fn guide_record_size(&self) -> u64 {
        self.guide_record_size
    }

    /// Original bounded guide records.
    #[must_use]
    pub fn guide_data(&self) -> &[u8] {
        &self.guide_data
    }
}

/// One typed special-ruler row.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Ruler {
    /// `RulerParallel`.
    Parallel {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Rotation in stored units.
        rotation: f64,
        /// Stored center.
        center: RulerPoint,
    },
    /// `RulerCurveParallel`.
    CurveParallel {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Optional raw curve kind.
        curve_kind: Option<i64>,
        /// Validated point payload.
        curve: RulerCurveData,
    },
    /// `RulerMultiCurve`.
    MultiCurve {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Optional raw curve kind.
        curve_kind: Option<i64>,
        /// Stored offset angle.
        offset_angle: f64,
        /// Stored center.
        center: RulerPoint,
        /// Validated point payload.
        curve: RulerCurveData,
    },
    /// `RulerEmit`.
    Emit {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Stored center.
        center: RulerPoint,
    },
    /// `RulerCurveEmit`.
    CurveEmit {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Optional raw curve kind.
        curve_kind: Option<i64>,
        /// Validated point payload.
        curve: RulerCurveData,
    },
    /// `RulerConcentricCircle`.
    ConcentricCircle {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Stored radii.
        radius: RulerPoint,
        /// Stored rotation.
        rotation: f64,
        /// Stored center.
        center: RulerPoint,
    },
    /// `RulerGuide`.
    Guide {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Raw horizontal flag.
        horizontal: i64,
        /// Stored center.
        center: RulerPoint,
    },
    /// `RulerPerspective` with its validated vanishing-point chain.
    Perspective {
        /// SQLite `MainId`.
        id: i64,
        /// Raw ruler flags.
        flags: i64,
        /// Raw perspective type.
        perspective_type: i64,
        /// Eye-level handle.
        eye_level_handle: RulerPoint,
        /// Move handle.
        move_handle: RulerPoint,
        /// Grid origin.
        grid_origin: RulerPoint,
        /// Raw grid flags.
        grid_flags: i64,
        /// Stored grid size.
        grid_size: f64,
        /// Stored near-camera value.
        camera_near: f64,
        /// Ordered vanishing-point rows.
        vanish_points: Vec<RulerVanishPoint>,
    },
    /// `RulerSymmetry`.
    Symmetry {
        /// SQLite `MainId`.
        id: i64,
        /// Raw snap flag.
        snap: i64,
        /// Stored line count.
        line_count: i64,
        /// Raw line-symmetry flag.
        line_symmetry: i64,
        /// Stored rotation.
        rotation: f64,
        /// Stored center.
        center: RulerPoint,
    },
}

impl Ruler {
    /// SQLite `MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        match self {
            Self::Parallel { id, .. }
            | Self::CurveParallel { id, .. }
            | Self::MultiCurve { id, .. }
            | Self::Emit { id, .. }
            | Self::CurveEmit { id, .. }
            | Self::ConcentricCircle { id, .. }
            | Self::Guide { id, .. }
            | Self::Perspective { id, .. }
            | Self::Symmetry { id, .. } => *id,
        }
    }

    /// Source table kind.
    #[must_use]
    pub const fn kind(&self) -> RulerKind {
        match self {
            Self::Parallel { .. } => RulerKind::Parallel,
            Self::CurveParallel { .. } => RulerKind::CurveParallel,
            Self::MultiCurve { .. } => RulerKind::MultiCurve,
            Self::Emit { .. } => RulerKind::Emit,
            Self::CurveEmit { .. } => RulerKind::CurveEmit,
            Self::ConcentricCircle { .. } => RulerKind::ConcentricCircle,
            Self::Guide { .. } => RulerKind::Guide,
            Self::Perspective { .. } => RulerKind::Perspective,
            Self::Symmetry { .. } => RulerKind::Symmetry,
        }
    }
}

/// Ruler references and typed special-ruler rows owned by one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerLayerData {
    layer_id: i64,
    canvas_id: i64,
    scope: Option<i64>,
    vector_object_id: Option<i64>,
    manager_id: Option<i64>,
    rulers: Vec<Ruler>,
}

impl RulerLayerData {
    /// Owning `Layer.MainId`.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Raw optional `Layer.RulerRange` value.
    #[must_use]
    pub const fn scope(&self) -> Option<i64> {
        self.scope
    }

    /// Referenced `VectorObjectList.MainId` for a vector ruler.
    #[must_use]
    pub const fn vector_object_id(&self) -> Option<i64> {
        self.vector_object_id
    }

    /// Referenced `SpecialRulerManager.MainId`.
    #[must_use]
    pub const fn manager_id(&self) -> Option<i64> {
        self.manager_id
    }

    /// Typed special rulers in manager-column and linked-list order.
    #[must_use]
    pub fn rulers(&self) -> &[Ruler] {
        &self.rulers
    }
}

impl Database {
    /// Discovers and validates every layer that owns ruler data.
    ///
    /// Schema differences and the underlying `Layer` query are handled
    /// internally. The result is ordered by layer ID and includes vector,
    /// special-ruler-manager, and ruler-range ownership.
    pub fn ruler_layers(&self, limits: Limits) -> Result<Vec<RulerLayerData>> {
        if self.schema().table("Layer").is_none() {
            return Ok(Vec::new());
        }
        self.require_column("Layer", "MainId")?;

        let mut predicates = Vec::new();
        if self.schema().has_column("Layer", "RulerVectorIndex") {
            predicates.push("COALESCE(RulerVectorIndex, 0) <> 0");
        }
        if self.schema().has_column("Layer", "SpecialRulerManager") {
            predicates.push("COALESCE(SpecialRulerManager, 0) <> 0");
        }
        if self.schema().has_column("Layer", "RulerRange") {
            predicates.push("RulerRange IS NOT NULL");
        }
        if predicates.is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT MainId FROM Layer WHERE {} ORDER BY MainId",
            predicates.join(" OR ")
        );
        let mut statement = self.connection().prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut layer_ids = Vec::new();
        while let Some(row) = rows.next()? {
            let layer_id: i64 = row.get(0)?;
            if layer_ids.last() == Some(&layer_id) {
                return Err(ruler_error(format!(
                    "ruler layer MainId {layer_id} is not unique"
                )));
            }
            if layer_ids.len() as u64 >= limits.max_layers() {
                return Err(Error::LimitExceeded {
                    resource: "ruler layers",
                    value: layer_ids.len() as u64 + 1,
                    limit: limits.max_layers(),
                });
            }
            layer_ids.push(layer_id);
        }
        drop(rows);
        drop(statement);

        let mut layers = Vec::with_capacity(layer_ids.len());
        for layer_id in layer_ids {
            let layer = self.ruler_layer(layer_id, limits)?.ok_or_else(|| {
                ruler_error(format!(
                    "layer {layer_id} was selected as a ruler owner but has no ruler data"
                ))
            })?;
            layers.push(layer);
        }
        Ok(layers)
    }

    /// Reads and validates all ruler references owned by one layer.
    ///
    /// An unknown layer or a layer without ruler columns/references returns
    /// `None`. Vector-ruler geometry remains available through the existing
    /// `vector_data_sources` and `read_vector_data` APIs.
    pub fn ruler_layer(&self, layer_id: i64, limits: Limits) -> Result<Option<RulerLayerData>> {
        if self.schema().table("Layer").is_none() {
            return Ok(None);
        }
        self.require_column("Layer", "MainId")?;
        self.require_column("Layer", "CanvasId")?;
        let vector_column = optional_layer_column(self, "RulerVectorIndex");
        let manager_column = optional_layer_column(self, "SpecialRulerManager");
        let scope_column = optional_layer_column(self, "RulerRange");
        if vector_column == "NULL" && manager_column == "NULL" && scope_column == "NULL" {
            return Ok(None);
        }

        let sql = format!(
            "SELECT CanvasId, {vector_column}, {manager_column}, {scope_column} \
             FROM Layer WHERE MainId = ?1 LIMIT 1"
        );
        let mut statement = self.connection().prepare(&sql)?;
        let mut rows = statement.query(params![layer_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let canvas_id: i64 = row.get(0)?;
        let vector_object_id = optional_positive(row.get(1)?, "RulerVectorIndex")?;
        let manager_id = optional_positive(row.get(2)?, "SpecialRulerManager")?;
        let scope: Option<i64> = row.get(3)?;
        drop(rows);
        drop(statement);

        if vector_object_id.is_none() && manager_id.is_none() && scope.is_none() {
            return Ok(None);
        }
        if canvas_id <= 0 {
            return Err(ruler_error("ruler layer has a non-positive CanvasId"));
        }
        if let Some(vector_object_id) = vector_object_id {
            validate_vector_ruler(self, vector_object_id, layer_id, canvas_id)?;
        }

        let mut rulers = Vec::new();
        if let Some(manager_id) = manager_id {
            let first = read_manager(self, manager_id, layer_id, canvas_id)?;
            for ((_, table, kind), first_id) in MANAGER_COLUMNS.iter().zip(first) {
                let Some(first_id) = first_id else {
                    continue;
                };
                let ids = read_chain_ids(self, table, first_id, layer_id, canvas_id, limits)?;
                for id in ids {
                    if rulers.len() as u64 >= limits.max_ruler_items() {
                        return Err(Error::LimitExceeded {
                            resource: "rulers",
                            value: rulers.len() as u64 + 1,
                            limit: limits.max_ruler_items(),
                        });
                    }
                    rulers.push(read_ruler(self, *kind, id, layer_id, canvas_id, limits)?);
                }
            }
        }

        Ok(Some(RulerLayerData {
            layer_id,
            canvas_id,
            scope,
            vector_object_id,
            manager_id,
            rulers,
        }))
    }
}

fn optional_layer_column(database: &Database, column: &str) -> &'static str {
    match column {
        "RulerVectorIndex" if database.schema().has_column("Layer", column) => "RulerVectorIndex",
        "SpecialRulerManager" if database.schema().has_column("Layer", column) => {
            "SpecialRulerManager"
        }
        "RulerRange" if database.schema().has_column("Layer", column) => "RulerRange",
        _ => "NULL",
    }
}

fn validate_vector_ruler(
    database: &Database,
    object_id: i64,
    layer_id: i64,
    canvas_id: i64,
) -> Result<()> {
    for column in ["MainId", "LayerId", "CanvasId"] {
        database.require_column("VectorObjectList", column)?;
    }
    let mut statement = database
        .connection()
        .prepare("SELECT LayerId, CanvasId FROM VectorObjectList WHERE MainId = ?1")?;
    let mut rows = statement.query(params![object_id])?;
    let Some(row) = rows.next()? else {
        return Err(ruler_error(format!(
            "RulerVectorIndex references missing VectorObjectList row {object_id}"
        )));
    };
    let actual_layer: i64 = row.get(0)?;
    let actual_canvas: i64 = row.get(1)?;
    if rows.next()?.is_some() {
        return Err(ruler_error(format!(
            "VectorObjectList MainId {object_id} is not unique"
        )));
    }
    if actual_layer != layer_id || actual_canvas != canvas_id {
        return Err(ruler_error(format!(
            "vector ruler {object_id} belongs to layer {actual_layer}, canvas {actual_canvas}"
        )));
    }
    Ok(())
}

fn read_manager(
    database: &Database,
    manager_id: i64,
    layer_id: i64,
    canvas_id: i64,
) -> Result<[Option<i64>; 9]> {
    for column in ["MainId", "LayerId", "CanvasId"] {
        database.require_column(MANAGER_TABLE, column)?;
    }
    for (column, _, _) in MANAGER_COLUMNS {
        database.require_column(MANAGER_TABLE, column)?;
    }
    let first_columns = MANAGER_COLUMNS
        .iter()
        .map(|(column, _, _)| *column)
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT LayerId, CanvasId, {first_columns} \
         FROM {MANAGER_TABLE} WHERE MainId = ?1"
    );
    let mut statement = database.connection().prepare(&sql)?;
    let mut rows = statement.query(params![manager_id])?;
    let Some(row) = rows.next()? else {
        return Err(ruler_error(format!(
            "SpecialRulerManager row {manager_id} does not exist"
        )));
    };
    let actual_layer: i64 = row.get(0)?;
    let actual_canvas: i64 = row.get(1)?;
    let mut first = [None; 9];
    for (index, (column, _, _)) in MANAGER_COLUMNS.iter().enumerate() {
        first[index] = optional_positive(row.get(index + 2)?, column)?;
    }
    if rows.next()?.is_some() {
        return Err(ruler_error(format!(
            "SpecialRulerManager MainId {manager_id} is not unique"
        )));
    }
    if actual_layer != layer_id || actual_canvas != canvas_id {
        return Err(ruler_error(format!(
            "ruler manager {manager_id} belongs to layer {actual_layer}, canvas {actual_canvas}"
        )));
    }
    Ok(first)
}

fn read_chain_ids(
    database: &Database,
    table: &str,
    first_id: i64,
    layer_id: i64,
    canvas_id: i64,
    limits: Limits,
) -> Result<Vec<i64>> {
    for column in ["MainId", "LayerId", "CanvasId", "NextIndex"] {
        database.require_column(table, column)?;
    }
    let sql = format!("SELECT LayerId, CanvasId, NextIndex FROM {table} WHERE MainId = ?1");
    let mut statement = database.connection().prepare(&sql)?;
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(first_id);
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(ruler_error(format!(
                "{table} linked list contains a cycle at {id}"
            )));
        }
        if ids.len() as u64 >= limits.max_ruler_items() {
            return Err(Error::LimitExceeded {
                resource: "rulers",
                value: ids.len() as u64 + 1,
                limit: limits.max_ruler_items(),
            });
        }
        let mut rows = statement.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Err(ruler_error(format!("{table} row {id} does not exist")));
        };
        let actual_layer: i64 = row.get(0)?;
        let actual_canvas: i64 = row.get(1)?;
        let next = optional_positive(row.get(2)?, "NextIndex")?;
        if rows.next()?.is_some() {
            return Err(ruler_error(format!("{table} MainId {id} is not unique")));
        }
        if actual_layer != layer_id || actual_canvas != canvas_id {
            return Err(ruler_error(format!(
                "{table} row {id} belongs to layer {actual_layer}, canvas {actual_canvas}"
            )));
        }
        ids.push(id);
        current = next;
    }

    let count_sql = format!("SELECT count(*) FROM {table} WHERE LayerId = ?1");
    let count: i64 = database
        .connection()
        .query_row(&count_sql, params![layer_id], |row| row.get(0))?;
    let count = u64::try_from(count).map_err(|_| ruler_error("negative ruler row count"))?;
    if count != ids.len() as u64 {
        return Err(ruler_error(format!(
            "{table} has {} rows unreachable from {first_id} for layer {layer_id}",
            count - ids.len() as u64
        )));
    }
    Ok(ids)
}

fn read_ruler(
    database: &Database,
    kind: RulerKind,
    id: i64,
    layer_id: i64,
    canvas_id: i64,
    limits: Limits,
) -> Result<Ruler> {
    match kind {
        RulerKind::Parallel => {
            let (snap, rotation, center) = query_values(
                database,
                "RulerParallel",
                id,
                "Snap, Rotate, CenterX, CenterY",
                |row| Ok((row.get(0)?, row.get(1)?, point(row.get(2)?, row.get(3)?)?)),
            )?;
            Ok(Ruler::Parallel {
                id,
                snap,
                rotation: finite(rotation, "parallel rotation")?,
                center,
            })
        }
        RulerKind::CurveParallel => {
            let (snap, curve_kind, data) = query_values(
                database,
                "RulerCurveParallel",
                id,
                "Snap, CurveKind, PointData",
                |row| Ok((row.get(0)?, row.get(1)?, required_blob(row.get_ref(2)?, 2)?)),
            )?;
            Ok(Ruler::CurveParallel {
                id,
                snap,
                curve_kind,
                curve: parse_curve_data(&data, limits)?,
            })
        }
        RulerKind::MultiCurve => {
            let (snap, curve_kind, offset_angle, center, data) = query_values(
                database,
                "RulerMultiCurve",
                id,
                "Snap, CurveKind, OffsetAngle, CenterX, CenterY, PointData",
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        point(row.get(3)?, row.get(4)?)?,
                        required_blob(row.get_ref(5)?, 5)?,
                    ))
                },
            )?;
            Ok(Ruler::MultiCurve {
                id,
                snap,
                curve_kind,
                offset_angle: finite(offset_angle, "multiple-curve offset angle")?,
                center,
                curve: parse_curve_data(&data, limits)?,
            })
        }
        RulerKind::Emit => {
            let (snap, center) =
                query_values(database, "RulerEmit", id, "Snap, CenterX, CenterY", |row| {
                    Ok((row.get(0)?, point(row.get(1)?, row.get(2)?)?))
                })?;
            Ok(Ruler::Emit { id, snap, center })
        }
        RulerKind::CurveEmit => {
            let (snap, curve_kind, data) = query_values(
                database,
                "RulerCurveEmit",
                id,
                "Snap, CurveKind, PointData",
                |row| Ok((row.get(0)?, row.get(1)?, required_blob(row.get_ref(2)?, 2)?)),
            )?;
            Ok(Ruler::CurveEmit {
                id,
                snap,
                curve_kind,
                curve: parse_curve_data(&data, limits)?,
            })
        }
        RulerKind::ConcentricCircle => {
            let (snap, radius, rotation, center) = query_values(
                database,
                "RulerConcentricCircle",
                id,
                "Snap, RadiusX, RadiusY, Rotate, CenterX, CenterY",
                |row| {
                    Ok((
                        row.get(0)?,
                        point(row.get(1)?, row.get(2)?)?,
                        row.get(3)?,
                        point(row.get(4)?, row.get(5)?)?,
                    ))
                },
            )?;
            Ok(Ruler::ConcentricCircle {
                id,
                snap,
                radius,
                rotation: finite(rotation, "concentric-circle rotation")?,
                center,
            })
        }
        RulerKind::Guide => {
            let (snap, horizontal, center) = query_values(
                database,
                "RulerGuide",
                id,
                "Snap, IsHorz, CenterX, CenterY",
                |row| Ok((row.get(0)?, row.get(1)?, point(row.get(2)?, row.get(3)?)?)),
            )?;
            Ok(Ruler::Guide {
                id,
                snap,
                horizontal,
                center,
            })
        }
        RulerKind::Perspective => {
            let (
                flags,
                perspective_type,
                eye_level_handle,
                move_handle,
                grid_origin,
                grid_flags,
                grid_size,
                camera_near,
                first_vanish,
            ) = query_values(
                database,
                "RulerPerspective",
                id,
                "Flag, PerspectiveType, EyeLevelHandleX, EyeLevelHandleY, \
                 MoveHandleX, MoveHandleY, GridOriginX, GridOriginY, GridFlag, \
                 GridSize, CameraNear, FirstVanishIndex",
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        point(row.get(2)?, row.get(3)?)?,
                        point(row.get(4)?, row.get(5)?)?,
                        point(row.get(6)?, row.get(7)?)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                    ))
                },
            )?;
            let first_vanish =
                optional_positive(first_vanish, "FirstVanishIndex")?.ok_or_else(|| {
                    ruler_error(format!("perspective ruler {id} has no vanishing point"))
                })?;
            let vanish_ids = read_chain_ids(
                database,
                "RulerVanishPoint",
                first_vanish,
                layer_id,
                canvas_id,
                limits,
            )?;
            let vanish_points = vanish_ids
                .into_iter()
                .map(|vanish_id| read_vanish_point(database, vanish_id, limits))
                .collect::<Result<Vec<_>>>()?;
            Ok(Ruler::Perspective {
                id,
                flags,
                perspective_type,
                eye_level_handle,
                move_handle,
                grid_origin,
                grid_flags,
                grid_size: finite(grid_size, "perspective grid size")?,
                camera_near: finite(camera_near, "perspective camera near")?,
                vanish_points,
            })
        }
        RulerKind::Symmetry => {
            let (snap, line_count, line_symmetry, rotation, center) = query_values(
                database,
                "RulerSymmetry",
                id,
                "Snap, LineNumber, LineSymmetry, Rotate, CenterX, CenterY",
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        point(row.get(4)?, row.get(5)?)?,
                    ))
                },
            )?;
            Ok(Ruler::Symmetry {
                id,
                snap,
                line_count,
                line_symmetry,
                rotation: finite(rotation, "symmetry rotation")?,
                center,
            })
        }
    }
}

fn query_values<T>(
    database: &Database,
    table: &str,
    id: i64,
    columns: &str,
    map: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<T> {
    for column in columns.split(", ") {
        database.require_column(table, column.trim())?;
    }
    let sql = format!("SELECT {columns} FROM {table} WHERE MainId = ?1 LIMIT 1");
    database
        .connection()
        .query_row(&sql, params![id], map)
        .map_err(Into::into)
}

fn read_vanish_point(database: &Database, id: i64, limits: Limits) -> Result<RulerVanishPoint> {
    let (flags, position, parallel_angle, guide_count, guide_size, guide_data) = query_values(
        database,
        "RulerVanishPoint",
        id,
        "Flag, VanishPointX, VanishPointY, ParallelAngle, GuideNumber, \
         GuideDataSize, Guide",
        |row| {
            Ok((
                row.get(0)?,
                point(row.get(1)?, row.get(2)?)?,
                row.get(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                optional_blob(row.get_ref(6)?, 6)?,
            ))
        },
    )?;
    let guide_count =
        u64::try_from(guide_count).map_err(|_| ruler_error("negative guide count"))?;
    let guide_record_size =
        u64::try_from(guide_size).map_err(|_| ruler_error("negative guide record size"))?;
    if guide_count > limits.max_ruler_items() {
        return Err(Error::LimitExceeded {
            resource: "ruler guides",
            value: guide_count,
            limit: limits.max_ruler_items(),
        });
    }
    let expected = guide_count
        .checked_mul(guide_record_size)
        .ok_or_else(|| ruler_error("guide byte count overflow"))?;
    if expected > limits.max_ruler_data_bytes() {
        return Err(Error::LimitExceeded {
            resource: "ruler data bytes",
            value: expected,
            limit: limits.max_ruler_data_bytes(),
        });
    }
    let guide_data = guide_data.unwrap_or_default();
    if guide_data.len() as u64 != expected {
        return Err(ruler_error(format!(
            "vanishing point {id} has {} guide bytes instead of {expected}",
            guide_data.len()
        )));
    }
    Ok(RulerVanishPoint {
        id,
        flags,
        position,
        parallel_angle: finite(parallel_angle, "vanishing-point angle")?,
        guide_count,
        guide_record_size,
        guide_data: guide_data.into_boxed_slice(),
    })
}

fn parse_curve_data(bytes: &[u8], limits: Limits) -> Result<RulerCurveData> {
    if bytes.len() as u64 > limits.max_ruler_data_bytes() {
        return Err(Error::LimitExceeded {
            resource: "ruler data bytes",
            value: bytes.len() as u64,
            limit: limits.max_ruler_data_bytes(),
        });
    }
    let mut parser = Parser::new(bytes);
    let header_size = parser.nonnegative_u32("curve header size")?;
    if header_size < 24 {
        return Err(ruler_error(format!(
            "curve header size {header_size} is below 24"
        )));
    }
    let point_count = parser.nonnegative_u64("curve point count")?;
    if point_count > limits.max_ruler_items() {
        return Err(Error::LimitExceeded {
            resource: "ruler curve points",
            value: point_count,
            limit: limits.max_ruler_items(),
        });
    }
    let metadata = [parser.i32()?, parser.i32()?, parser.i32()?, parser.i32()?];
    parser.take(header_size as usize - 24)?;
    let expected_points = point_count
        .checked_mul(20)
        .ok_or_else(|| ruler_error("curve point byte count overflow"))?;
    if parser.remaining() as u64 != expected_points {
        return Err(ruler_error(format!(
            "curve point data has {} bytes instead of {expected_points}",
            parser.remaining()
        )));
    }
    let mut points = Vec::with_capacity(point_count as usize);
    for _ in 0..point_count {
        points.push(RulerCurvePoint {
            position: point(parser.f64()?, parser.f64()?)?,
            thickness: parser.i32()?,
        });
    }
    Ok(RulerCurveData {
        header_size,
        metadata,
        points,
        raw: bytes.into(),
    })
}

fn required_blob(value: ValueRef<'_>, column: usize) -> rusqlite::Result<Vec<u8>> {
    match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(bytes.to_vec()),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            "ruler data".to_owned(),
            value.data_type(),
        )),
    }
}

fn optional_blob(value: ValueRef<'_>, column: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(Some(bytes.to_vec())),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            "ruler data".to_owned(),
            value.data_type(),
        )),
    }
}

fn point(x: f64, y: f64) -> rusqlite::Result<RulerPoint> {
    if x.is_finite() && y.is_finite() {
        Ok(RulerPoint { x, y })
    } else {
        Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Real,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-finite ruler point",
            )),
        ))
    }
}

fn finite(value: f64, name: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ruler_error(format!("{name} is not finite")))
    }
}

fn optional_positive(value: Option<i64>, name: &str) -> Result<Option<i64>> {
    match value {
        None | Some(0) => Ok(None),
        Some(value) if value > 0 => Ok(Some(value)),
        Some(value) => Err(ruler_error(format!("{name} value {value} is negative"))),
    }
}

fn ruler_error(reason: impl Into<String>) -> Error {
    Error::InvalidRuler {
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
            .ok_or_else(|| ruler_error("ruler offset overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| ruler_error("ruler payload is truncated"))?;
        self.offset = end;
        Ok(value)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes were taken"),
        ))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes were taken"),
        ))
    }

    fn nonnegative_u32(&mut self, name: &str) -> Result<u32> {
        u32::try_from(self.i32()?).map_err(|_| ruler_error(format!("{name} is negative")))
    }

    fn nonnegative_u64(&mut self, name: &str) -> Result<u64> {
        Ok(u64::from(self.nonnegative_u32(name)?))
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn base_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Layer (
                    MainId INTEGER, CanvasId INTEGER, RulerVectorIndex INTEGER,
                    SpecialRulerManager INTEGER, RulerRange INTEGER
                 );
                 CREATE TABLE VectorObjectList (
                    MainId INTEGER, CanvasId INTEGER, LayerId INTEGER, VectorData BLOB
                 );
                 CREATE TABLE SpecialRulerManager (
                    MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
                    FirstParallel INTEGER, FirstCurveParallel INTEGER,
                    FirstMultiCurve INTEGER, FirstEmit INTEGER,
                    FirstCurveEmit INTEGER, FirstConcentricCircle INTEGER,
                    FirstGuide INTEGER, FirstPerspective INTEGER,
                    FirstSymmetry INTEGER
                 );
                 CREATE TABLE RulerGuide (
                    MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
                    NextIndex INTEGER, Snap INTEGER, IsHorz INTEGER,
                    CenterX REAL, CenterY REAL
                 );",
            )
            .unwrap();
        connection
    }

    #[test]
    fn validates_a_vector_ruler_reference() {
        let connection = base_database();
        connection
            .execute("INSERT INTO Layer VALUES (1, 2, 3, NULL, 1)", [])
            .unwrap();
        connection
            .execute("INSERT INTO VectorObjectList VALUES (3, 2, 1, X'00')", [])
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        let ruler = database.ruler_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(ruler.vector_object_id(), Some(3));
        assert_eq!(ruler.scope(), Some(1));
        assert!(ruler.rulers().is_empty());
    }

    #[test]
    fn reads_and_orders_a_guide_chain() {
        let connection = base_database();
        connection
            .execute("INSERT INTO Layer VALUES (1, 2, NULL, 7, 0)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO SpecialRulerManager VALUES (
                    7, 2, 1, 0, 0, 0, 0, 0, 0, 10, 0, 0
                 )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO RulerGuide VALUES (10, 2, 1, 11, 1, 1, 2.0, 3.0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO RulerGuide VALUES (11, 2, 1, 0, 0, 0, 4.0, 5.0)",
                [],
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        let ruler = database.ruler_layer(1, Limits::default()).unwrap().unwrap();
        assert_eq!(
            ruler.rulers().iter().map(Ruler::id).collect::<Vec<_>>(),
            [10, 11]
        );
        assert!(matches!(
            ruler.rulers()[0],
            Ruler::Guide {
                horizontal: 1,
                center,
                ..
            } if center == RulerPoint { x: 2.0, y: 3.0 }
        ));
    }

    #[test]
    fn discovers_ruler_layers_without_exposing_sql() {
        let connection = base_database();
        connection
            .execute("INSERT INTO Layer VALUES (3, 2, NULL, NULL, NULL)", [])
            .unwrap();
        connection
            .execute("INSERT INTO Layer VALUES (1, 2, 4, NULL, 1)", [])
            .unwrap();
        connection
            .execute("INSERT INTO VectorObjectList VALUES (4, 2, 1, X'00')", [])
            .unwrap();
        connection
            .execute("INSERT INTO Layer VALUES (2, 2, NULL, 7, NULL)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO SpecialRulerManager VALUES (
                    7, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0
                 )",
                [],
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();

        let layers = database.ruler_layers(Limits::default()).unwrap();
        assert_eq!(
            layers
                .iter()
                .map(RulerLayerData::layer_id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(layers[0].vector_object_id(), Some(4));
        assert_eq!(layers[1].manager_id(), Some(7));
        assert!(matches!(
            database.ruler_layers(Limits::default().with_max_layers(1)),
            Err(Error::LimitExceeded {
                resource: "ruler layers",
                value: 2,
                limit: 1,
            })
        ));
    }

    #[test]
    fn ruler_layer_discovery_tolerates_absent_optional_schema() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE Layer (MainId INTEGER, CanvasId INTEGER)")
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        assert!(database.ruler_layers(Limits::default()).unwrap().is_empty());
    }

    #[test]
    fn parses_curve_points_and_preserves_header_words() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&24_i32.to_be_bytes());
        bytes.extend_from_slice(&1_i32.to_be_bytes());
        for value in [20_i32, 1, 0, 1] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&2.5_f64.to_be_bytes());
        bytes.extend_from_slice(&3.5_f64.to_be_bytes());
        bytes.extend_from_slice(&7_i32.to_be_bytes());
        let curve = parse_curve_data(&bytes, Limits::default()).unwrap();
        assert_eq!(curve.metadata(), [20, 1, 0, 1]);
        assert_eq!(curve.points()[0].position(), RulerPoint { x: 2.5, y: 3.5 });
        assert_eq!(curve.points()[0].thickness(), 7);
        assert_eq!(curve.raw(), bytes);
    }

    #[test]
    fn rejects_cycles_unreachable_rows_and_limits() {
        let connection = base_database();
        connection
            .execute("INSERT INTO Layer VALUES (1, 2, NULL, 7, 0)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO SpecialRulerManager VALUES (
                    7, 2, 1, 0, 0, 0, 0, 0, 0, 10, 0, 0
                 )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO RulerGuide VALUES (10, 2, 1, 10, 1, 1, 2.0, 3.0)",
                [],
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        assert!(matches!(
            database.ruler_layer(1, Limits::default()),
            Err(Error::InvalidRuler { .. })
        ));
        assert!(matches!(
            database.ruler_layer(1, Limits::default().with_max_ruler_items(0)),
            Err(Error::LimitExceeded {
                resource: "rulers",
                ..
            })
        ));
    }
}
