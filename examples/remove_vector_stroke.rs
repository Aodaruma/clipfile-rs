//! Removes one supported vector stroke without regenerating render caches.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select one VectorObjectList row and its zero-based stroke index.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err(
            "usage: cargo run --features write --example remove_vector_stroke -- \
             <input.clip> <new-output.clip> <layer-id> <vector-row-id> <stroke-index>"
                .into(),
        );
    }
    let layer_id = args[3].parse::<i64>()?;
    let row_id = args[4].parse::<i64>()?;
    let stroke_index = args[5].parse::<usize>()?;
    let limits = Limits::default();

    // Resolve a typed source and let the writer revalidate row ownership.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let database = clip.open_database()?;
    let source = database
        .vector_data_sources(layer_id, limits)?
        .into_iter()
        .find(|source| source.id() == row_id)
        .ok_or("the requested vector row does not belong to this layer")?;

    // Delete exactly one validated record; removing the last yields an empty body.
    let mut writer = clip.writer()?;
    let removed_points = writer.remove_vector_stroke(&source, stroke_index, limits)?;

    // The new container is fully validated, but cached previews remain untouched.
    writer.write_to_path(&args[2])?;
    println!("removed stroke {stroke_index} containing {removed_points} point(s)");
    Ok(())
}
