//! Replaces the complete opaque external body of one validated vector row.

use std::{env, fs};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse paths plus the layer and VectorObjectList row to update.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err(
            "usage: cargo run --features write --example edit_vector_body -- \
             <input.clip> <output.clip> <layer-id> <vector-row-id> <body.bin>"
                .into(),
        );
    }
    let layer_id = args[3].parse::<i64>()?;
    let row_id = args[4].parse::<i64>()?;
    let limits = Limits::default();

    // Resolve the typed SQLite reference before entering the rewrite session.
    let mut clip = ClipFile::open(fs::File::open(&args[1])?)?;
    let database = clip.open_database()?;
    let source = database
        .vector_data_sources(layer_id, limits)?
        .into_iter()
        .find(|source| source.id() == row_id)
        .ok_or("the requested vector row does not belong to this layer")?;

    // The internal stroke encoding is opaque, so replace one complete body.
    let replacement = fs::read(&args[5])?;
    let mut writer = clip.writer()?;
    writer.replace_vector_data_body(&source, replacement, limits)?;

    // Rebuild offsets and validate the newly created CLIP file.
    writer.write_to_path(&args[2])?;
    println!("replaced opaque vector row {row_id}");
    Ok(())
}
