//! Translates every point in one supported existing vector-data row.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the row address and integer canvas-space translation.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 7 {
        return Err(
            "usage: cargo run --features write --example translate_vector -- \
             <input.clip> <output.clip> <layer-id> <vector-row-id> <dx> <dy>"
                .into(),
        );
    }
    let layer_id = args[3].parse::<i64>()?;
    let row_id = args[4].parse::<i64>()?;
    let delta_x = args[5].parse::<i32>()?;
    let delta_y = args[6].parse::<i32>()?;
    let limits = Limits::default();

    // Resolve the typed row so the writer can revalidate its external identifier.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let database = clip.open_database()?;
    let source = database
        .vector_data_sources(layer_id, limits)?
        .into_iter()
        .find(|source| source.id() == row_id)
        .ok_or("the requested vector row does not belong to this layer")?;

    // Patch only the validated point positions and bounding boxes.
    let mut writer = clip.writer()?;
    let summary = writer.translate_vector_data(&source, delta_x, delta_y, limits)?;
    writer.write_to_path(&args[2])?;
    println!(
        "translated {} point(s) in {} stroke(s)",
        summary.points(),
        summary.strokes()
    );
    Ok(())
}
