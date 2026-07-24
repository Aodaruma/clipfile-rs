//! Clones an image-cel track and replaces its key sequence.

use std::{env, fs::File};

use clipfile::{ClipFile, ImageCelTrackCloneOptions, ImageCelTrackKeyframe, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Each requested key is supplied as three arguments: time, numeric value,
    // and cel tag. At least one triple is required.
    let args = env::args().collect::<Vec<_>>();
    if args.len() < 9 || (args.len() - 6) % 3 != 0 {
        return Err(
            "usage: cargo run --features \"write,animation\" --example clone_image_cel_track -- \
             <input.clip> <new-output.clip> <template-track-id> <timeline-id> \
             <target-layer-id> <time-60hz> <numeric-value> <cel-tag> [...]"
                .into(),
        );
    }
    let template_track_id = args[3].parse::<i64>()?;
    let timeline_id = args[4].parse::<i64>()?;
    let target_layer_id = args[5].parse::<i64>()?;
    let mut keyframes = Vec::new();
    for fields in args[6..].chunks_exact(3) {
        keyframes.push(ImageCelTrackKeyframe::new(
            fields[0].parse::<f32>()?,
            fields[1].parse::<u32>()?,
            &fields[2],
        ));
    }
    let options = ImageCelTrackCloneOptions::new(keyframes);

    // Clone complete opaque mixer graphs from a verified kind-2000 template.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let summary = writer.clone_image_cel_track_from_template(
        template_track_id,
        timeline_id,
        target_layer_id,
        &options,
        Limits::default(),
    )?;

    // Fresh row, UUID, and external identities are committed as one new file.
    writer.write_to_path(&args[2])?;
    println!(
        "created image-cel track {} with {} key(s)",
        summary.track_id(),
        options.keyframes().len()
    );
    Ok(())
}
