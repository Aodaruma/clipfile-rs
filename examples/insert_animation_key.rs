//! Inserts one key by copying optional fields from a compatible existing key.

use std::{env, fs::File};

use clipfile::{AnimationCurveKeyframeInsert, ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // An axis of "-" addresses a scalar curve. The template supplies every
    // optional per-key field required by the existing curve representation.
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 10 {
        return Err(
            "usage: cargo run --features \"write,animation\" --example insert_animation_key -- \
             <input.clip> <new-output.clip> <track-id> <curve-kind> <axis-or--> \
             <template-key-index> <insert-index> <time-60hz> <value>"
                .into(),
        );
    }
    let track_id = args[3].parse::<i64>()?;
    let axis = (args[5] != "-").then_some(args[5].as_str());
    let template_index = args[6].parse::<usize>()?;
    let insert_index = args[7].parse::<usize>()?;
    let time_60hz = args[8].parse::<f32>()?;
    let value = args[9].parse::<f32>()?;
    let limits = Limits::default();

    // Decode the selected curve and copy one complete key as the field template.
    let mut clip = ClipFile::open(File::open(&args[1])?)?;
    let database = clip.open_database()?;
    let animation = clip
        .read_animation(&database, limits)?
        .ok_or("the document has no selected animation timeline")?;
    let key = animation
        .animation_tracks()
        .iter()
        .find(|track| track.id() == track_id)
        .and_then(|track| {
            track
                .curves()
                .iter()
                .find(|curve| curve.kind() == args[4] && curve.axis() == axis)
        })
        .and_then(|curve| curve.keyframes().get(template_index))
        .cloned()
        .ok_or("the requested template key was not found")?;
    drop(database);

    // Supply exactly the optional arrays represented by the template key.
    let mut insertion = AnimationCurveKeyframeInsert::new(time_60hz, value);
    if let Some(tag) = key.tag() {
        insertion = insertion.with_tag(tag);
    }
    if let Some(interpolation) = key.interpolation() {
        insertion = insertion.with_interpolation(interpolation);
    }
    match (key.left_slope(), key.right_slope()) {
        (Some(left), Some(right)) => insertion = insertion.with_slopes(left, right),
        (None, None) => {}
        _ => return Err("the key contains only one slope and cannot be cloned safely".into()),
    }
    if let Some(revise_constant) = key.revise_constant() {
        insertion = insertion.with_revise_constant(revise_constant);
    }

    // Insert primary and matching secondary records together, then validate output.
    let mut writer = clip.writer()?;
    writer.insert_animation_curve_keyframe(
        track_id,
        &args[4],
        axis,
        insert_index,
        &insertion,
        limits,
    )?;
    writer.write_to_path(&args[2])?;
    println!("inserted key {insert_index} into {}/{axis:?}", args[4]);
    Ok(())
}
