//! Adds one text object by cloning a validated existing object's attributes.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the layer/template address and compatible replacement text.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err(
            "usage: cargo run --features write --example add_text_object -- \
             <input.clip> <output.clip> <layer-id> <template-index> <text>"
                .into(),
        );
    }
    let layer_id = args[3].parse::<i64>()?;
    let template_index = args[4].parse::<usize>()?;

    // Clone opaque style/layout attributes and allocate a fresh object ID.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let added = writer.database().add_text_object_from_template(
        layer_id,
        template_index,
        &args[5],
        Limits::default(),
    )?;

    // Write a new validated file; existing output paths are never overwritten.
    writer.write_to_path(&args[2])?;
    println!(
        "added text object {} with identifier {}",
        added.object_index(),
        added.identifier()
    );
    Ok(())
}
