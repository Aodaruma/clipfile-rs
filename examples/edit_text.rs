//! Replaces one existing text object while preserving encoded character widths.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the source/output paths and the existing text-object address.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err("usage: cargo run --features write --example edit_text -- \
             <input.clip> <output.clip> <layer-id> <object-index> <replacement>"
            .into());
    }
    let layer_id = args[3].parse::<i64>()?;
    let object_index = args[4].parse::<usize>()?;

    // Edit only the validated text column in the writer's private database.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let original = writer.database().replace_text_object_text(
        layer_id,
        object_index,
        &args[5],
        Limits::default(),
    )?;

    // Write a new validated CLIP file without overwriting an existing path.
    writer.write_to_path(&args[2])?;
    println!("replaced {original:?} with {:?}", args[5]);
    Ok(())
}
