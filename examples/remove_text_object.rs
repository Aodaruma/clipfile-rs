//! Removes one text object while retaining a structurally valid text layer.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Address one object using the order returned by Database::text_layer.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: cargo run --features write --example remove_text_object -- \
             <input.clip> <new-output.clip> <layer-id> <object-index>"
                .into(),
        );
    }
    let layer_id = args[3].parse::<i64>()?;
    let object_index = args[4].parse::<usize>()?;

    // Remove all paired string/style records in one database update.
    // Index zero is promoted from the next object; the final object is rejected.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let removed =
        writer
            .database()
            .remove_text_object(layer_id, object_index, Limits::default())?;

    // Keep the input untouched and write only to a new path.
    writer.write_to_path(&args[2])?;
    println!("removed text object {object_index}: {:?}", removed.text());
    Ok(())
}
