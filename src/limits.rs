/// Configurable safety limits applied while parsing untrusted files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    max_identifier_size: u64,
    max_top_level_chunks: u64,
    max_blocks_per_external: u64,
    max_database_size: u64,
    max_decompressed_block_size: u64,
    max_raster_bytes: u64,
    max_preview_bytes: u64,
    max_vector_data_bytes: u64,
    max_vector_objects: u64,
    max_text_bytes: u64,
    max_text_objects: u64,
    max_correction_bytes: u64,
    max_correction_items: u64,
    max_ruler_data_bytes: u64,
    max_ruler_items: u64,
    max_animation_bytes: u64,
    max_animation_items: u64,
    max_time_lapse_blob_bytes: u64,
    max_time_lapse_items: u64,
    max_write_external_body_size: u64,
    max_cmc_nodes: u64,
    max_layers: u64,
    max_layer_tree_depth: u64,
    max_canvas_dimension: u32,
}

impl Limits {
    /// Returns the maximum accepted file or external-object identifier size.
    #[must_use]
    pub const fn max_identifier_size(&self) -> u64 {
        self.max_identifier_size
    }

    /// Returns the maximum number of top-level chunks.
    #[must_use]
    pub const fn max_top_level_chunks(&self) -> u64 {
        self.max_top_level_chunks
    }

    /// Returns the maximum number of blocks in one external object.
    #[must_use]
    pub const fn max_blocks_per_external(&self) -> u64 {
        self.max_blocks_per_external
    }

    /// Returns the maximum accepted SQLite payload size.
    #[must_use]
    pub const fn max_database_size(&self) -> u64 {
        self.max_database_size
    }

    /// Returns the maximum accepted decompressed size of one block.
    #[must_use]
    pub const fn max_decompressed_block_size(&self) -> u64 {
        self.max_decompressed_block_size
    }

    /// Returns the maximum allocation for one decoded raster image.
    #[must_use]
    pub const fn max_raster_bytes(&self) -> u64 {
        self.max_raster_bytes
    }

    /// Returns the maximum accepted encoded size of one canvas preview.
    #[must_use]
    pub const fn max_preview_bytes(&self) -> u64 {
        self.max_preview_bytes
    }

    /// Returns the maximum accepted size of one raw vector-data body.
    #[must_use]
    pub const fn max_vector_data_bytes(&self) -> u64 {
        self.max_vector_data_bytes
    }

    /// Returns the maximum number of vector-data rows accepted for one layer.
    #[must_use]
    pub const fn max_vector_objects(&self) -> u64 {
        self.max_vector_objects
    }

    /// Returns the maximum accepted total text-layer payload size.
    #[must_use]
    pub const fn max_text_bytes(&self) -> u64 {
        self.max_text_bytes
    }

    /// Returns the maximum number of text objects accepted for one layer.
    #[must_use]
    pub const fn max_text_objects(&self) -> u64 {
        self.max_text_objects
    }

    /// Returns the maximum accepted size of one correction-layer payload.
    #[must_use]
    pub const fn max_correction_bytes(&self) -> u64 {
        self.max_correction_bytes
    }

    /// Returns the maximum accepted count for correction channels, stops, and points.
    #[must_use]
    pub const fn max_correction_items(&self) -> u64 {
        self.max_correction_items
    }

    /// Returns the maximum accepted size of one ruler data payload.
    #[must_use]
    pub const fn max_ruler_data_bytes(&self) -> u64 {
        self.max_ruler_data_bytes
    }

    /// Returns the maximum accepted ruler, curve-point, and guide count.
    #[must_use]
    pub const fn max_ruler_items(&self) -> u64 {
        self.max_ruler_items
    }

    /// Returns the maximum accepted encoded or decoded animation payload size.
    #[must_use]
    pub const fn max_animation_bytes(&self) -> u64 {
        self.max_animation_bytes
    }

    /// Returns the maximum accepted count for animation rows and arrays.
    #[must_use]
    pub const fn max_animation_items(&self) -> u64 {
        self.max_animation_items
    }

    /// Returns the maximum accepted compressed or decoded time-lapse blob size.
    #[must_use]
    pub const fn max_time_lapse_blob_bytes(&self) -> u64 {
        self.max_time_lapse_blob_bytes
    }

    /// Returns the maximum accepted time-lapse manager, record, and blob count.
    #[must_use]
    pub const fn max_time_lapse_items(&self) -> u64 {
        self.max_time_lapse_items
    }

    /// Returns the maximum accepted size of one replacement external body.
    #[must_use]
    pub const fn max_write_external_body_size(&self) -> u64 {
        self.max_write_external_body_size
    }

    /// Returns the maximum number of nodes accepted from one `.cmc` file.
    #[must_use]
    pub const fn max_cmc_nodes(&self) -> u64 {
        self.max_cmc_nodes
    }

    /// Returns the maximum number of layers accepted by the document model.
    #[must_use]
    pub const fn max_layers(&self) -> u64 {
        self.max_layers
    }

    /// Returns the maximum accepted layer-tree depth.
    #[must_use]
    pub const fn max_layer_tree_depth(&self) -> u64 {
        self.max_layer_tree_depth
    }

    /// Returns the maximum accepted width or height of a canvas.
    #[must_use]
    pub const fn max_canvas_dimension(&self) -> u32 {
        self.max_canvas_dimension
    }

    /// Sets the maximum accepted identifier size.
    #[must_use]
    pub const fn with_max_identifier_size(mut self, value: u64) -> Self {
        self.max_identifier_size = value;
        self
    }

    /// Sets the maximum number of top-level chunks.
    #[must_use]
    pub const fn with_max_top_level_chunks(mut self, value: u64) -> Self {
        self.max_top_level_chunks = value;
        self
    }

    /// Sets the maximum number of blocks in one external object.
    #[must_use]
    pub const fn with_max_blocks_per_external(mut self, value: u64) -> Self {
        self.max_blocks_per_external = value;
        self
    }

    /// Sets the maximum accepted SQLite payload size.
    #[must_use]
    pub const fn with_max_database_size(mut self, value: u64) -> Self {
        self.max_database_size = value;
        self
    }

    /// Sets the maximum decompressed size of one block.
    #[must_use]
    pub const fn with_max_decompressed_block_size(mut self, value: u64) -> Self {
        self.max_decompressed_block_size = value;
        self
    }

    /// Sets the maximum allocation for one decoded raster image.
    #[must_use]
    pub const fn with_max_raster_bytes(mut self, value: u64) -> Self {
        self.max_raster_bytes = value;
        self
    }

    /// Sets the maximum accepted encoded size of one canvas preview.
    #[must_use]
    pub const fn with_max_preview_bytes(mut self, value: u64) -> Self {
        self.max_preview_bytes = value;
        self
    }

    /// Sets the maximum accepted size of one raw vector-data body.
    #[must_use]
    pub const fn with_max_vector_data_bytes(mut self, value: u64) -> Self {
        self.max_vector_data_bytes = value;
        self
    }

    /// Sets the maximum number of vector-data rows accepted for one layer.
    #[must_use]
    pub const fn with_max_vector_objects(mut self, value: u64) -> Self {
        self.max_vector_objects = value;
        self
    }

    /// Sets the maximum accepted total text-layer payload size.
    #[must_use]
    pub const fn with_max_text_bytes(mut self, value: u64) -> Self {
        self.max_text_bytes = value;
        self
    }

    /// Sets the maximum number of text objects accepted for one layer.
    #[must_use]
    pub const fn with_max_text_objects(mut self, value: u64) -> Self {
        self.max_text_objects = value;
        self
    }

    /// Sets the maximum accepted size of one correction-layer payload.
    #[must_use]
    pub const fn with_max_correction_bytes(mut self, value: u64) -> Self {
        self.max_correction_bytes = value;
        self
    }

    /// Sets the maximum accepted correction channel, stop, and point count.
    #[must_use]
    pub const fn with_max_correction_items(mut self, value: u64) -> Self {
        self.max_correction_items = value;
        self
    }

    /// Sets the maximum accepted size of one ruler data payload.
    #[must_use]
    pub const fn with_max_ruler_data_bytes(mut self, value: u64) -> Self {
        self.max_ruler_data_bytes = value;
        self
    }

    /// Sets the maximum accepted ruler, curve-point, and guide count.
    #[must_use]
    pub const fn with_max_ruler_items(mut self, value: u64) -> Self {
        self.max_ruler_items = value;
        self
    }

    /// Sets the maximum accepted encoded or decoded animation payload size.
    #[must_use]
    pub const fn with_max_animation_bytes(mut self, value: u64) -> Self {
        self.max_animation_bytes = value;
        self
    }

    /// Sets the maximum accepted count for animation rows and arrays.
    #[must_use]
    pub const fn with_max_animation_items(mut self, value: u64) -> Self {
        self.max_animation_items = value;
        self
    }

    /// Sets the maximum accepted compressed or decoded time-lapse blob size.
    #[must_use]
    pub const fn with_max_time_lapse_blob_bytes(mut self, value: u64) -> Self {
        self.max_time_lapse_blob_bytes = value;
        self
    }

    /// Sets the maximum accepted time-lapse manager, record, and blob count.
    #[must_use]
    pub const fn with_max_time_lapse_items(mut self, value: u64) -> Self {
        self.max_time_lapse_items = value;
        self
    }

    /// Sets the maximum accepted size of one replacement external body.
    #[must_use]
    pub const fn with_max_write_external_body_size(mut self, value: u64) -> Self {
        self.max_write_external_body_size = value;
        self
    }

    /// Sets the maximum number of nodes accepted from one `.cmc` file.
    #[must_use]
    pub const fn with_max_cmc_nodes(mut self, value: u64) -> Self {
        self.max_cmc_nodes = value;
        self
    }

    /// Sets the maximum number of layers accepted by the document model.
    #[must_use]
    pub const fn with_max_layers(mut self, value: u64) -> Self {
        self.max_layers = value;
        self
    }

    /// Sets the maximum accepted layer-tree depth.
    #[must_use]
    pub const fn with_max_layer_tree_depth(mut self, value: u64) -> Self {
        self.max_layer_tree_depth = value;
        self
    }

    /// Sets the maximum accepted width or height of a canvas.
    #[must_use]
    pub const fn with_max_canvas_dimension(mut self, value: u32) -> Self {
        self.max_canvas_dimension = value;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_identifier_size: 64 * 1024,
            max_top_level_chunks: 1_000_000,
            max_blocks_per_external: 1_000_000,
            max_database_size: 512 * 1024 * 1024,
            max_decompressed_block_size: 16 * 1024 * 1024,
            max_raster_bytes: 1024 * 1024 * 1024,
            max_preview_bytes: 256 * 1024 * 1024,
            max_vector_data_bytes: 256 * 1024 * 1024,
            max_vector_objects: 1_000_000,
            max_text_bytes: 64 * 1024 * 1024,
            max_text_objects: 1_000_000,
            max_correction_bytes: 64 * 1024 * 1024,
            max_correction_items: 1_000_000,
            max_ruler_data_bytes: 64 * 1024 * 1024,
            max_ruler_items: 1_000_000,
            max_animation_bytes: 256 * 1024 * 1024,
            max_animation_items: 1_000_000,
            max_time_lapse_blob_bytes: 128 * 1024 * 1024,
            max_time_lapse_items: 1_000_000,
            max_write_external_body_size: 512 * 1024 * 1024,
            max_cmc_nodes: 1_000_000,
            max_layers: 1_000_000,
            max_layer_tree_depth: 4_096,
            max_canvas_dimension: 1_000_000,
        }
    }
}
