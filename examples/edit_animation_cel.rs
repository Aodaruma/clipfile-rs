//! Renames one existing animation cel key without rebuilding unknown fields.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the track/key address and the replacement cel tag.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err("usage: cargo run --features \"write,animation\" \
             --example edit_animation_cel -- \
             <input.clip> <output.clip> <track-id> <key-index> <cel-tag>"
            .into());
    }
    let track_id = args[3].parse::<i64>()?;
    let key_index = args[4].parse::<usize>()?;

    // Patch the validated primary/secondary Tag records and matching current value.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let mut writer = clip.writer()?;
    let original =
        writer.replace_animation_cel_tag(track_id, key_index, &args[5], Limits::default())?;

    // Write and structurally re-open the result through the safe path API.
    writer.write_to_path(&args[2])?;
    println!("replaced cel tag {original:?} with {:?}", args[5]);
    Ok(())
}
