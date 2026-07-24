//! Prints a selected timeline, validated track chains, curves, values, and cel selections.

use std::{env, fs::File};

use clipfile::ClipFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This example accepts one CLIP document and inspects its selected timeline.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features animation --example inspect_animation -- <input.clip>",
    )?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features animation --example inspect_animation -- <input.clip>"
                .into(),
        );
    }

    // Animation decoding combines SQLite metadata with bounded external mixer data.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let Some(animation) = clip.read_animation(&database, clip.limits())? else {
        println!("animation: none");
        return Ok(());
    };
    let timeline = animation.timeline();
    println!(
        "timeline {}: name={:?}, {} fps, frames {}..={}, current {:?}",
        timeline.id(),
        timeline.name(),
        timeline.frame_rate(),
        timeline.start_frame(),
        timeline.end_frame(),
        timeline.current_frame()
    );

    // Generic tracks retain unknown numeric kinds and expose all decoded curve groups.
    for track in animation.animation_tracks() {
        println!(
            "track {}: kind={}, layer={:?}, values={}, primary curves={}, secondary curves={}",
            track.id(),
            track.kind().raw(),
            track.layer_id(),
            track.values().len(),
            track.curves().len(),
            track.secondary_curves().len()
        );
        for curve in track.curves() {
            println!(
                "  primary curve {:?}/{:?}: {} key(s)",
                curve.kind(),
                curve.axis(),
                curve.keyframes().len()
            );
        }
        for curve in track.secondary_curves() {
            println!(
                "  secondary curve {:?}/{:?}: {} key(s)",
                curve.kind(),
                curve.axis(),
                curve.keyframes().len()
            );
        }
    }

    // Cel tracks map display-frame positions to selected image-cel tags.
    for track in animation.tracks() {
        let current_cel = timeline
            .current_frame()
            .and_then(|frame| track.cel_at_frame(frame, timeline.frame_rate()));
        println!(
            "cel track {}: layer {}, {} key(s), current cel {:?}",
            track.id(),
            track.layer_id(),
            track.keyframes().len(),
            current_cel
        );
    }
    Ok(())
}
