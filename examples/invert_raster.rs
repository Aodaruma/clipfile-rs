//! Replaces a complete existing raster through row-major semantic pixels.
//!
//! Color/value channels are inverted while alpha is preserved.

use std::{env, fs::File};

use clipfile::{ClipFile, RasterImage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select one existing render raster and a new output path.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features \"write,raster\" --example invert_raster -- <input.clip> <new-output.clip> <layer-id>",
    )?;
    let output = arguments.next().ok_or(
        "usage: cargo run --features \"write,raster\" --example invert_raster -- <input.clip> <new-output.clip> <layer-id>",
    )?;
    let layer_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features \"write,raster\" --example invert_raster -- <input.clip> <new-output.clip> <layer-id>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features \"write,raster\" --example invert_raster -- <input.clip> <new-output.clip> <layer-id>".into(),
        );
    }

    // Decode the existing tile grid into semantic row-major pixels.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let source = database
        .layer_raster_source(layer_id)?
        .ok_or_else(|| format!("layer {layer_id} has no render raster"))?;
    let mut raster: RasterImage = clip.decode_raster(&database, &source)?;
    let width = raster.width();
    let height = raster.height();
    let format = raster.format();
    drop(database);

    // Apply one format-independent operation; independent alpha is preserved.
    for mut pixel in raster.pixel_iter_mut() {
        pixel.invert();
    }

    // Rebuild changed tiles with the compatible checksum selected internally.
    let mut writer = clip.writer()?;
    let raster = writer.replace_layer_raster(layer_id, raster)?;
    let output_summary = writer.write_to_path(output)?;
    println!(
        "wrote {width}x{height} {format:?}: {}/{} tile(s) changed; output {} bytes",
        raster.changed_tiles(),
        raster.total_tiles(),
        output_summary.output_file_size()
    );
    Ok(())
}
