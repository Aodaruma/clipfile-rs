use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, MAIN_DB};

use crate::{Database, Error, Limits, Result};

/// One node in a standalone CLIP STUDIO `.cmc` page-management file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CmcNode {
    id: i64,
    kind: i64,
    next_id: Option<i64>,
    first_child_id: Option<i64>,
    selected_id: Option<i64>,
    canvas_index: i64,
    page_flags: i64,
    raw_link_path: Option<String>,
    page_file_name: Option<String>,
}

impl CmcNode {
    /// Positive `CanvasNode.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Opaque `CanvasNode.Type` value.
    #[must_use]
    pub const fn kind(&self) -> i64 {
        self.kind
    }

    /// Next node in the stored sibling chain.
    #[must_use]
    pub const fn next_id(&self) -> Option<i64> {
        self.next_id
    }

    /// First child in the stored child chain.
    #[must_use]
    pub const fn first_child_id(&self) -> Option<i64> {
        self.first_child_id
    }

    /// Selected child/node reference, when present.
    #[must_use]
    pub const fn selected_id(&self) -> Option<i64> {
        self.selected_id
    }

    /// Opaque `CanvasNode.CanvasIndex` value.
    #[must_use]
    pub const fn canvas_index(&self) -> i64 {
        self.canvas_index
    }

    /// Opaque `CanvasNode.PageFlag` value.
    #[must_use]
    pub const fn page_flags(&self) -> i64 {
        self.page_flags
    }

    /// Original `CanvasNode.LinkPath` text.
    #[must_use]
    pub fn raw_link_path(&self) -> Option<&str> {
        self.raw_link_path.as_deref()
    }

    /// Traversal-safe page file name decoded from the observed `.:name` form.
    ///
    /// Unknown, absolute, nested, or parent-traversing link forms return
    /// `None`; the original value remains available through
    /// [`Self::raw_link_path`].
    #[must_use]
    pub fn page_file_name(&self) -> Option<&str> {
        self.page_file_name.as_deref()
    }
}

/// Validated, read-only access to a standalone CLIP STUDIO `.cmc` file.
pub struct CmcFile {
    database: Database,
    source_path: Option<PathBuf>,
    internal_version: String,
    root_node_id: i64,
    max_canvas_file_index: i64,
    nodes: Vec<CmcNode>,
    node_indexes: BTreeMap<i64, usize>,
    children: BTreeMap<i64, Vec<i64>>,
}

impl CmcFile {
    /// Opens and validates a `.cmc` SQLite file from a filesystem path.
    pub fn open(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let path = path.as_ref();
        let mut file = File::open(path)?;
        let database = read_database(&mut file, limits)?;
        Self::from_database(database, Some(path.to_path_buf()), limits)
    }

    /// Reads and validates a `.cmc` SQLite file from a seekable stream.
    ///
    /// Page references remain available, but [`Self::page_path`] returns
    /// `None` because a stream has no source directory.
    pub fn from_reader<R: Read + Seek>(mut reader: R, limits: Limits) -> Result<Self> {
        let database = read_database(&mut reader, limits)?;
        Self::from_database(database, None, limits)
    }

    fn from_database(
        database: Database,
        source_path: Option<PathBuf>,
        limits: Limits,
    ) -> Result<Self> {
        database.quick_check()?;
        for column in [
            "ProjectInternalVersion",
            "ProjectRootCanvasNode",
            "MaxCanvasFileIndex",
        ] {
            database.require_column("Project", column)?;
        }
        for column in [
            "MainId",
            "Type",
            "NextIndex",
            "FirstChildIndex",
            "SelectedIndex",
            "CanvasIndex",
            "PageFlag",
            "LinkPath",
        ] {
            database.require_column("CanvasNode", column)?;
        }

        let project_rows = database.row_count("Project")?;
        if project_rows != 1 {
            return Err(cmc_error(format!(
                "Project has {project_rows} rows instead of one"
            )));
        }
        let (internal_version, root_node_id, max_canvas_file_index) =
            database.connection().query_row(
                "SELECT ProjectInternalVersion, ProjectRootCanvasNode, MaxCanvasFileIndex \
                 FROM Project",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if root_node_id <= 0 {
            return Err(cmc_error("ProjectRootCanvasNode is not positive"));
        }
        if max_canvas_file_index < 0 {
            return Err(cmc_error("MaxCanvasFileIndex is negative"));
        }

        let node_count = database.row_count("CanvasNode")?;
        if node_count > limits.max_cmc_nodes() {
            return Err(Error::LimitExceeded {
                resource: "CMC nodes",
                value: node_count,
                limit: limits.max_cmc_nodes(),
            });
        }
        let mut statement = database.connection().prepare(
            "SELECT MainId, Type, NextIndex, FirstChildIndex, SelectedIndex, \
                    CanvasIndex, PageFlag, LinkPath \
             FROM CanvasNode ORDER BY MainId",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;

        let mut nodes_by_id = BTreeMap::new();
        for row in rows {
            let (
                id,
                kind,
                next_id,
                first_child_id,
                selected_id,
                canvas_index,
                page_flags,
                raw_link_path,
            ) = row?;
            if id <= 0 {
                return Err(cmc_error(format!("CanvasNode MainId {id} is not positive")));
            }
            let node = CmcNode {
                id,
                kind,
                next_id: optional_reference(next_id, "NextIndex")?,
                first_child_id: optional_reference(first_child_id, "FirstChildIndex")?,
                selected_id: optional_reference(selected_id, "SelectedIndex")?,
                canvas_index,
                page_flags,
                page_file_name: raw_link_path
                    .as_deref()
                    .and_then(observed_page_file_name)
                    .map(str::to_owned),
                raw_link_path,
            };
            if nodes_by_id.insert(id, node).is_some() {
                return Err(cmc_error(format!("duplicate CanvasNode MainId {id}")));
            }
        }
        drop(statement);

        if !nodes_by_id.contains_key(&root_node_id) {
            return Err(cmc_error(format!(
                "root CanvasNode {root_node_id} does not exist"
            )));
        }
        for node in nodes_by_id.values() {
            for (name, reference) in [
                ("NextIndex", node.next_id),
                ("FirstChildIndex", node.first_child_id),
                ("SelectedIndex", node.selected_id),
            ] {
                if let Some(reference) =
                    reference.filter(|reference| !nodes_by_id.contains_key(reference))
                {
                    return Err(cmc_error(format!(
                        "CanvasNode {} {name} references missing node {reference}",
                        node.id
                    )));
                }
            }
        }

        let mut children = nodes_by_id
            .keys()
            .map(|id| (*id, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let mut parents = BTreeMap::new();
        for parent in nodes_by_id.values() {
            let mut current = parent.first_child_id;
            let mut sibling_chain = BTreeSet::new();
            while let Some(child_id) = current {
                if !sibling_chain.insert(child_id) {
                    return Err(cmc_error(format!(
                        "CanvasNode sibling chain under {} contains a cycle at {child_id}",
                        parent.id
                    )));
                }
                if let Some(previous) = parents
                    .insert(child_id, parent.id)
                    .filter(|previous| *previous != parent.id)
                {
                    return Err(cmc_error(format!(
                        "CanvasNode {child_id} has parents {previous} and {}",
                        parent.id
                    )));
                }
                children
                    .get_mut(&parent.id)
                    .expect("all node IDs were inserted")
                    .push(child_id);
                current = nodes_by_id
                    .get(&child_id)
                    .expect("references were validated")
                    .next_id;
            }
        }
        if let Some(parent) = parents.get(&root_node_id) {
            return Err(cmc_error(format!(
                "root CanvasNode {root_node_id} is a child of {parent}"
            )));
        }

        let mut ordered_ids = Vec::with_capacity(nodes_by_id.len());
        let mut visited = BTreeSet::new();
        let mut stack = vec![root_node_id];
        while let Some(id) = stack.pop() {
            if !visited.insert(id) {
                return Err(cmc_error(format!("CanvasNode tree revisits node {id}")));
            }
            ordered_ids.push(id);
            for child in children
                .get(&id)
                .expect("all node IDs were inserted")
                .iter()
                .rev()
            {
                stack.push(*child);
            }
        }
        if visited.len() != nodes_by_id.len() {
            return Err(cmc_error(format!(
                "{} CanvasNode rows are unreachable from root {root_node_id}",
                nodes_by_id.len() - visited.len()
            )));
        }

        let nodes = ordered_ids
            .into_iter()
            .map(|id| nodes_by_id.remove(&id).expect("ordered IDs exist"))
            .collect::<Vec<_>>();
        let node_indexes = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id, index))
            .collect();

        Ok(Self {
            database,
            source_path,
            internal_version,
            root_node_id,
            max_canvas_file_index,
            nodes,
            node_indexes,
            children,
        })
    }

    /// Underlying read-only SQLite database for advanced queries.
    #[must_use]
    pub const fn database(&self) -> &Database {
        &self.database
    }

    /// Source path used by [`Self::open`], if any.
    #[must_use]
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// `Project.ProjectInternalVersion`.
    #[must_use]
    pub fn internal_version(&self) -> &str {
        &self.internal_version
    }

    /// Validated root `CanvasNode` ID.
    #[must_use]
    pub const fn root_node_id(&self) -> i64 {
        self.root_node_id
    }

    /// Non-negative `Project.MaxCanvasFileIndex`.
    #[must_use]
    pub const fn max_canvas_file_index(&self) -> i64 {
        self.max_canvas_file_index
    }

    /// All nodes in validated depth-first tree order.
    #[must_use]
    pub fn nodes(&self) -> &[CmcNode] {
        &self.nodes
    }

    /// Looks up a node by its positive `MainId`.
    #[must_use]
    pub fn node(&self, id: i64) -> Option<&CmcNode> {
        self.node_indexes.get(&id).map(|index| &self.nodes[*index])
    }

    /// Ordered children for a known node, or `None` for an unknown ID.
    #[must_use]
    pub fn children_of(&self, id: i64) -> Option<&[i64]> {
        self.children.get(&id).map(Vec::as_slice)
    }

    /// Nodes with a stored page link, in validated tree order.
    pub fn page_nodes(&self) -> impl Iterator<Item = &CmcNode> {
        self.nodes
            .iter()
            .filter(|node| node.raw_link_path.is_some())
    }

    /// Resolves a traversal-safe page link relative to the `.cmc` directory.
    ///
    /// Returns `None` for stream-backed files, unknown node IDs, and link
    /// forms other than the observed single-file `.:name` representation.
    #[must_use]
    pub fn page_path(&self, node_id: i64) -> Option<PathBuf> {
        let directory = self.source_path.as_deref()?.parent()?;
        let file_name = self.node(node_id)?.page_file_name()?;
        Some(directory.join(file_name))
    }
}

fn read_database<R: Read + Seek>(reader: &mut R, limits: Limits) -> Result<Database> {
    let start = reader.stream_position()?;
    let end = reader.seek(SeekFrom::End(0))?;
    let size = end.checked_sub(start).ok_or(Error::OffsetOverflow)?;
    if size > limits.max_database_size() {
        return Err(Error::PayloadTooLarge {
            size,
            limit: limits.max_database_size(),
        });
    }
    let size = usize::try_from(size).map_err(|_| Error::PayloadTooLarge {
        size,
        limit: usize::MAX as u64,
    })?;
    reader.seek(SeekFrom::Start(start))?;
    let source = reader.take(size as u64);
    let mut connection = Connection::open_in_memory()?;
    connection.deserialize_read_exact(MAIN_DB, source, size, true)?;
    Database::from_connection(connection)
}

fn optional_reference(value: i64, name: &str) -> Result<Option<i64>> {
    match value {
        0 => Ok(None),
        value if value > 0 => Ok(Some(value)),
        value => Err(cmc_error(format!(
            "CanvasNode {name} value {value} is negative"
        ))),
    }
}

fn observed_page_file_name(link: &str) -> Option<&str> {
    let file_name = link.strip_prefix(".:")?;
    if file_name.is_empty()
        || matches!(file_name, "." | "..")
        || file_name
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b':' | 0))
    {
        return None;
    }
    Some(file_name)
}

fn cmc_error(reason: impl Into<String>) -> Error {
    Error::InvalidCmc {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn cmc_database(cycle: bool, unsafe_link: bool) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE Project (
                    ProjectInternalVersion TEXT,
                    ProjectRootCanvasNode INTEGER,
                    MaxCanvasFileIndex INTEGER
                 );
                 CREATE TABLE CanvasNode (
                    MainId INTEGER,
                    Type INTEGER,
                    NextIndex INTEGER,
                    FirstChildIndex INTEGER,
                    SelectedIndex INTEGER,
                    CanvasIndex INTEGER,
                    PageFlag INTEGER,
                    LinkPath TEXT
                 );
                 INSERT INTO Project VALUES ('1.1.0', 1, 2);
                 INSERT INTO CanvasNode VALUES (1, 0, 0, 2, 2, 0, 0, NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO CanvasNode VALUES (2, 2, ?1, 0, 0, 0, 0, ?2)",
                rusqlite::params![
                    if cycle { 2 } else { 3 },
                    if unsafe_link {
                        ".:../outside.clip"
                    } else {
                        ".:page0001.clip"
                    }
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO CanvasNode VALUES (3, 2, 0, 0, 0, 0, 0, '.:page0002.clip')",
                [],
            )
            .unwrap();
        connection
    }

    #[test]
    fn reads_and_resolves_a_valid_page_index() {
        let cmc = CmcFile::from_database(
            Database::from_connection(cmc_database(false, false)).unwrap(),
            Some(PathBuf::from("project/book.cmc")),
            Limits::default(),
        )
        .unwrap();

        assert_eq!(cmc.internal_version(), "1.1.0");
        assert_eq!(cmc.root_node_id(), 1);
        assert_eq!(cmc.max_canvas_file_index(), 2);
        assert_eq!(cmc.children_of(1), Some([2, 3].as_slice()));
        assert_eq!(
            cmc.page_nodes()
                .map(|node| node.page_file_name().unwrap())
                .collect::<Vec<_>>(),
            ["page0001.clip", "page0002.clip"]
        );
        assert_eq!(
            cmc.page_path(2).as_deref(),
            Some(Path::new("project/page0001.clip"))
        );
    }

    #[test]
    fn preserves_unknown_links_without_resolving_them() {
        let cmc = CmcFile::from_database(
            Database::from_connection(cmc_database(false, true)).unwrap(),
            Some(PathBuf::from("project/book.cmc")),
            Limits::default(),
        )
        .unwrap();

        assert_eq!(
            cmc.node(2).unwrap().raw_link_path(),
            Some(".:../outside.clip")
        );
        assert_eq!(cmc.node(2).unwrap().page_file_name(), None);
        assert_eq!(cmc.page_path(2), None);
    }

    #[test]
    fn rejects_cycles_and_node_limit_overruns() {
        assert!(matches!(
            CmcFile::from_database(
                Database::from_connection(cmc_database(true, false)).unwrap(),
                None,
                Limits::default(),
            ),
            Err(Error::InvalidCmc { .. })
        ));
        assert!(matches!(
            CmcFile::from_database(
                Database::from_connection(cmc_database(false, false)).unwrap(),
                None,
                Limits::default().with_max_cmc_nodes(2),
            ),
            Err(Error::LimitExceeded {
                resource: "CMC nodes",
                ..
            })
        ));
    }
}
