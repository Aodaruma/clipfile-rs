//! Removes one key while keeping primary and secondary curve arrays aligned.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // An axis of "-" addresses a scalar curve.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 7 {
        return Err(
            "usage: cargo run --features \"write,animation\" --example remove_animation_key -- \
             <input.clip> <new-output.clip> <track-id> <curve-kind> <axis-or--> <key-index>"
                .into(),
        );
    }
    let track_id = args[3].parse::<i64>()?;
    let axis = (args[5] != "-").then_some(args[5].as_str());
    let key_index = args[6].parse::<usize>()?;

    // Remove every represented per-key field in one conservative operation.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let removed = writer.remove_animation_curve_keyframe(
        track_id,
        &args[4],
        axis,
        key_index,
        Limits::default(),
    )?;

    // Empty curves are intentionally rejected; at least one key must remain.
    writer.write_to_path(&args[2])?;
    println!(
        "removed key {key_index}: time={}, value={}",
        removed.time_60hz(),
        removed.value()
    );
    Ok(())
}
