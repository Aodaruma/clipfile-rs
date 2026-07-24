//! Clones one complete animation track into an untracked compatible layer.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Address an existing template, its target timeline, and an untracked layer.
    // The library cannot infer kind compatibility: choose the same semantic
    // layer kind as the template track's original layer.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err("usage: cargo run --features \"write,animation\" \
             --example clone_animation_track -- \
             <input.clip> <output.clip> <template-track-id> \
             <timeline-id> <target-layer-id>"
            .into());
    }
    let template_track_id = args[3].parse::<i64>()?;
    let timeline_id = args[4].parse::<i64>()?;
    let target_layer_id = args[5].parse::<i64>()?;

    // Clone the full row and both mixer bodies under independent generated IDs.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let cloned = writer.clone_animation_track_from_template(
        template_track_id,
        timeline_id,
        target_layer_id,
        Limits::default(),
    )?;

    // Emit a new file; the path API refuses to overwrite an existing output.
    writer.write_to_path(&args[2])?;
    println!(
        "cloned track {} as {} for layer {}",
        cloned.template_track_id(),
        cloned.track_id(),
        cloned.layer_id()
    );
    Ok(())
}
