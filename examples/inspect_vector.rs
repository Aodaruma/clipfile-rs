//! Resolves one layer's vector references and reads their bounded opaque bodies.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Vector references are queried by their owning layer ID.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>",
    )?;
    let layer_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>"
                .into(),
        );
    }

    // The database returns typed ownership metadata without interpreting vector bytes.
    let limits = Limits::default();
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let sources = database.vector_data_sources(layer_id, limits)?;
    println!("layer {layer_id}: {} vector object(s)", sources.len());

    // Each external body is resolved through the validated SQLite external index.
    for source in &sources {
        let bytes = clip.read_vector_data(&database, source, limits)?;
        println!(
            "vector object {}: canvas {}, layer {}, {} raw bytes",
            source.id(),
            source.canvas_id(),
            source.layer_id(),
            bytes.len()
        );
    }
    Ok(())
}
