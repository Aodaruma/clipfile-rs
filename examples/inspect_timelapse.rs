//! Streams and validates time-lapse metadata and internal WebP frame indexes.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Time-lapse data is optional, so a valid file may report `none`.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features timelapse --example inspect_timelapse -- <input.clip>",
    )?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features timelapse --example inspect_timelapse -- <input.clip>"
                .into(),
        );
    }

    // Load the validated manager -> record -> blob chains from SQLite.
    let limits = Limits::default();
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let Some(time_lapse) = database.time_lapse(limits)? else {
        println!("time-lapse: none");
        return Ok(());
    };
    println!("time-lapse managers: {}", time_lapse.managers().len());

    // Frame indexing streams decoded blobs and skips embedded WebP payload bytes.
    for manager in time_lapse.managers() {
        println!(
            "manager {}: canvas {}, {} record(s)",
            manager.id(),
            manager.canvas_id(),
            manager.records().len()
        );
        for record in manager.records() {
            let frames = clip.read_time_lapse_frame_index(&database, record, limits)?;
            println!(
                "  record {}: encoder {:?}, {} blob(s), {} decoded bytes, {} frame(s)",
                record.id(),
                record.encoder_name(),
                record.blobs().len(),
                record.decoded_size(),
                frames.len()
            );
            for frame in frames.iter().take(3) {
                println!(
                    "    frame {}: kind={}, dimensions={:?}, delta origin={:?}, encoded bytes={}",
                    frame.sequence(),
                    frame.kind().known_name().unwrap_or("unknown"),
                    frame.webp_dimensions(),
                    frame.delta_origin(),
                    frame.encoded_size()
                );
            }
            if frames.len() > 3 {
                println!("    ... {} more frame(s)", frames.len() - 3);
            }
        }
    }
    Ok(())
}
