//! Unlinks and deletes one animation Track row from a validated timeline.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select the owning timeline explicitly so the chain can be revalidated.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: cargo run --features \"write,animation\" --example remove_animation_track -- \
             <input.clip> <new-output.clip> <timeline-id> <track-id>"
                .into(),
        );
    }
    let timeline_id = args[3].parse::<i64>()?;
    let track_id = args[4].parse::<i64>()?;

    // Repair either the timeline head or the predecessor's next link.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let summary = writer.remove_animation_track(timeline_id, track_id, Limits::default())?;

    // Opaque mixer bodies are intentionally retained as conservative orphans.
    writer.write_to_path(&args[2])?;
    println!(
        "removed track {}; predecessor={:?}, successor={:?}",
        summary.track_id(),
        summary.previous_track_id(),
        summary.next_track_id()
    );
    Ok(())
}
