//! Clones a plain raster layer and supplies a complete new pixel image.
//!
//! For a visible, dependency-free example, the template pixels are decoded
//! and RGB or grayscale values are inverted before creating the new layer.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits, RasterImage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select a compatible leaf template, its destination parent, and a name.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err(
            "usage: cargo run --features \"write,raster\" --example clone_raster_layer -- \
             <input.clip> <new-output.clip> <template-layer-id> <parent-layer-id> <layer-name>"
                .into(),
        );
    }
    let template_layer_id = args[3].parse::<i64>()?;
    let parent_layer_id = args[4].parse::<i64>()?;
    let limits = Limits::default();

    // Decode the template's base render into semantic row-major pixels.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let database = clip.open_database()?;
    let source = database
        .layer_raster_source(template_layer_id)?
        .ok_or("the template layer has no render raster")?;
    let mut raster: RasterImage = clip.decode_raster(&database, &source)?;
    drop(database);

    // Prepare visibly different pixels; the common operation preserves alpha.
    for mut pixel in raster.pixel_iter_mut() {
        pixel.invert();
    }

    // Clone unknown metadata from the template while assigning fresh identities.
    let mut writer = clip.writer()?;
    let layer_id = writer.clone_raster_layer_from_template_image(
        template_layer_id,
        parent_layer_id,
        &args[5],
        raster,
        limits,
    )?;

    // Emit a separate, reopened-and-validated CLIP file.
    writer.write_to_path(&args[2])?;
    println!("created raster layer {layer_id}");
    Ok(())
}
