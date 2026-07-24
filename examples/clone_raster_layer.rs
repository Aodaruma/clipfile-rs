//! Clones a plain raster layer and supplies a complete new pixel image.
//!
//! For a visible, dependency-free example, the template pixels are decoded
//! and RGB or grayscale values are inverted before creating the new layer.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits, PixelFormat};

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
    let image = clip.decode_raster(&database, &source)?;
    let format = image.format();
    let mut pixels = image.into_pixels();
    drop(database);

    // Prepare visibly different pixels without changing the alpha channel.
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

    // Clone unknown metadata from the template while assigning fresh identities.
    let mut writer = clip.writer()?;
    let layer_id = writer.clone_raster_layer_from_template(
        template_layer_id,
        parent_layer_id,
        &args[5],
        format,
        pixels,
        limits,
    )?;

    // Emit a separate, reopened-and-validated CLIP file.
    writer.write_to_path(&args[2])?;
    println!("created raster layer {layer_id}");
    Ok(())
}
