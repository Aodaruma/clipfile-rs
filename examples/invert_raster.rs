//! Replaces a complete existing raster through row-major semantic pixels.
//!
//! RGB channels are inverted while alpha is preserved. Grayscale rasters,
//! including supported layer masks, invert their single channel.

use std::{env, fs::File};

use clipfile::{BlockChecksumMode, ClipFile, PixelFormat};

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
    let image = clip.decode_raster(&database, &source)?;
    let width = image.width();
    let height = image.height();
    let format = image.format();
    let mut pixels = image.into_pixels();
    drop(database);

    // Apply a format-aware edit instead of changing native tile padding.
    match format {
        PixelFormat::Rgba8 => {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[0] = u8::MAX - pixel[0];
                pixel[1] = u8::MAX - pixel[1];
                pixel[2] = u8::MAX - pixel[2];
            }
        }
        PixelFormat::Gray8 => {
            for value in &mut pixels {
                *value = u8::MAX - *value;
            }
        }
        _ => return Err("this example does not support the decoded pixel format".into()),
    }

    // Rebuild only changed tiles and write a separately validated CLIP file.
    let mut writer = clip.writer()?;
    let raster = writer.replace_layer_raster_pixels(
        layer_id,
        format,
        pixels,
        BlockChecksumMode::CspCompatible,
    )?;
    let output_summary = writer.write_to_path(output)?;
    println!(
        "wrote {width}x{height} {format:?}: {}/{} tile(s) changed; output {} bytes",
        raster.changed_tiles(),
        raster.total_tiles(),
        output_summary.output_file_size()
    );
    Ok(())
}
