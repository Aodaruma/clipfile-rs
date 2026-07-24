//! Decodes one layer raster and writes it as a dependency-free Netpbm PAM image.

use std::{
    env,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
};

use clipfile::{ClipFile, PixelFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The output must be a new `.pam` path; this example never overwrites a file.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features raster --example export_raster -- <input.clip> <layer-id> <new-output.pam>",
    )?;
    let layer_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features raster --example export_raster -- <input.clip> <layer-id> <new-output.pam>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    let output = arguments.next().ok_or(
        "usage: cargo run --features raster --example export_raster -- <input.clip> <layer-id> <new-output.pam>",
    )?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features raster --example export_raster -- <input.clip> <layer-id> <new-output.pam>".into(),
        );
    }

    // Resolve the layer's render mipmap and assemble its tiles into row-major pixels.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let source = database
        .layer_raster_source(layer_id)?
        .ok_or_else(|| format!("layer {layer_id} has no render raster"))?;
    let image = clip.decode_raster(&database, &source)?;

    // PAM supports both RGBA and grayscale while keeping the example dependency-free.
    let (depth, tuple_type) = match image.format() {
        PixelFormat::Rgba8 => (4, "RGB_ALPHA"),
        PixelFormat::Gray8 => (1, "GRAYSCALE"),
        _ => return Err("this example does not support the decoded pixel format".into()),
    };
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut output = BufWriter::new(output);
    write!(
        output,
        "P7\nWIDTH {}\nHEIGHT {}\nDEPTH {depth}\nMAXVAL 255\nTUPLTYPE {tuple_type}\nENDHDR\n",
        image.width(),
        image.height()
    )?;
    output.write_all(image.pixels())?;
    output.flush()?;

    // The state explains whether pixels came from external tiles or default fill metadata.
    println!(
        "wrote {}x{} {:?} raster ({:?})",
        image.width(),
        image.height(),
        image.format(),
        image.data_state()
    );
    Ok(())
}
