//! Decodes one layer raster and exports it as PNG through image-rs.

use std::{
    env,
    fs::{File, OpenOptions},
    io::{BufWriter, Write as _},
};

use clipfile::ClipFile;
use image::ImageFormat;

const USAGE: &str = "usage: cargo run --features image --example export_raster -- \
     <input.clip> <layer-id> <new-output.png>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select an existing layer and a new PNG path.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(USAGE)?;
    let layer_id = arguments
        .next()
        .ok_or(USAGE)?
        .to_string_lossy()
        .parse::<i64>()?;
    let output = arguments.next().ok_or(USAGE)?;
    if arguments.next().is_some() {
        return Err(USAGE.into());
    }

    // Resolve the layer's render mipmap and assemble its tiles into row-major pixels.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let source = database
        .layer_raster_source(layer_id)?
        .ok_or_else(|| format!("layer {layer_id} has no render raster"))?;
    let raster = clip.decode_raster(&database, &source)?;
    let width = raster.width();
    let height = raster.height();
    let format = raster.format();
    let data_state = raster.data_state();
    drop(database);

    // Convert without copying the pixel buffer, then let image-rs encode PNG.
    let image = raster.into_dynamic_image();
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut output = BufWriter::new(output);
    image.write_to(&mut output, ImageFormat::Png)?;
    output.flush()?;

    // The state explains whether pixels came from external tiles or default fill metadata.
    println!("wrote {width}x{height} {format:?} raster ({data_state:?})");
    Ok(())
}
