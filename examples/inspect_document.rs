//! Loads the high-level document model and prints canvas, layer, and tree metadata.

use std::{env, fs::File};

use clipfile::ClipFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse exactly one input path so accidental extra arguments are not ignored.
    let mut arguments = env::args_os().skip(1);
    let input = arguments
        .next()
        .ok_or("usage: cargo run --features sqlite --example inspect_document -- <input.clip>")?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features sqlite --example inspect_document -- <input.clip>".into(),
        );
    }

    // Build the validated, high-level project/canvas/layer representation.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let document = clip.read_document()?;
    println!(
        "project version: {}; canvases: {}; layer rows: {}",
        document.project().internal_version(),
        document.canvases().len(),
        document.layers().len()
    );

    // Canvas records expose dimensions and the root/current layer references.
    for canvas in document.canvases() {
        println!(
            "canvas {}: {}x{} at {} dpi; root layer {}; current layer {:?}",
            canvas.id(),
            canvas.width(),
            canvas.height(),
            canvas.resolution(),
            canvas.root_layer_id(),
            canvas.current_layer_id()
        );
    }

    // Layer records retain raw kinds while providing common interpreted flags.
    for layer in document.layers() {
        println!(
            "layer {}: name={:?}, kind={}, visible={}, folder={}, clipped={}, opacity={:.3}",
            layer.id(),
            layer.name(),
            layer.kind().raw(),
            layer.is_visible(),
            layer.is_folder(),
            layer.is_clipped(),
            layer.opacity_fraction()
        );
    }

    // Each canvas tree reports validated reachability without recursive model types.
    for tree in document.layer_trees() {
        println!(
            "tree for canvas {}: {} reachable layers, root children {:?}, unreachable {:?}",
            tree.canvas_id(),
            tree.reachable_layer_count(),
            tree.root_children(),
            tree.unreachable_layer_ids()
        );
    }
    Ok(())
}
