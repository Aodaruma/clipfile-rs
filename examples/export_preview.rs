//! Extracts one canvas's validated embedded PNG preview into a new file.

use std::{
    env,
    fs::{File, OpenOptions},
    io::Write,
};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Canvas IDs are shown by `inspect_document`; the output path must be new.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features sqlite --example export_preview -- <input.clip> <canvas-id> <new-output.png>",
    )?;
    let canvas_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features sqlite --example export_preview -- <input.clip> <canvas-id> <new-output.png>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    let output = arguments.next().ok_or(
        "usage: cargo run --features sqlite --example export_preview -- <input.clip> <canvas-id> <new-output.png>",
    )?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features sqlite --example export_preview -- <input.clip> <canvas-id> <new-output.png>".into(),
        );
    }

    // CanvasPreview validates allocation bounds and PNG IHDR dimensions.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let Some(preview) = database.canvas_preview(canvas_id, Limits::default())? else {
        return Err(format!("canvas {canvas_id} has no embedded preview").into());
    };
    if !preview.is_png() {
        return Err(format!(
            "canvas {canvas_id} uses unsupported preview type {}",
            preview.image_type()
        )
        .into());
    }

    // Use create_new so an example invocation cannot overwrite an existing image.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    output.write_all(preview.data())?;
    output.flush()?;

    // The dimensions have already been cross-checked against the PNG header.
    println!(
        "wrote canvas {} preview: {}x{}, {} bytes",
        preview.canvas_id(),
        preview.width(),
        preview.height(),
        preview.data().len()
    );
    Ok(())
}
