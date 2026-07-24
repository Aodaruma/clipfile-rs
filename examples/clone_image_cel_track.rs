//! Clones an image-cel track and replaces its key sequence.

use std::{env, fs::File};

use clipfile::{ClipFile, ImageCelTrackCloneOptions, Limits};

const USAGE: &str = "usage: cargo run --features \"write,animation\" --example clone_image_cel_track -- \
     <input.clip> <new-output.clip> <template-track-id> <timeline-id> \
     <target-layer-id> <time-60hz> <cel-tag> [...]";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the fixed document/track arguments first.
    let mut arguments = env::args().skip(1);
    let input = next_argument(&mut arguments, "input CLIP path")?;
    let output = next_argument(&mut arguments, "new output CLIP path")?;
    let template_track_id = next_argument(&mut arguments, "template track ID")?.parse::<i64>()?;
    let timeline_id = next_argument(&mut arguments, "timeline ID")?.parse::<i64>()?;
    let target_layer_id = next_argument(&mut arguments, "target layer ID")?.parse::<i64>()?;

    // Each remaining group describes one typed image-cel keyframe.
    let keyframes = parse_keyframes(arguments)?;
    let options = ImageCelTrackCloneOptions::from_timed_cels(keyframes)?;

    // Clone complete opaque mixer graphs from a verified kind-2000 template.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let mut writer = clip.writer()?;
    let summary = writer.clone_image_cel_track_from_template(
        template_track_id,
        timeline_id,
        target_layer_id,
        &options,
        Limits::default(),
    )?;

    // Fresh row, UUID, and external identities are committed as one new file.
    writer.write_to_path(output)?;
    println!(
        "created image-cel track {} with {} key(s)",
        summary.track_id(),
        options.keyframes().len()
    );
    Ok(())
}

fn next_argument(
    arguments: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    arguments
        .next()
        .ok_or_else(|| format!("missing {name}; {USAGE}").into())
}

fn parse_keyframes(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Vec<(f32, String)>, Box<dyn std::error::Error>> {
    let mut keyframes = Vec::new();

    // Read named values instead of indexing anonymous three-element chunks.
    while let Some(time_text) = arguments.next() {
        let key_number = keyframes.len() + 1;
        let cel_tag = arguments
            .next()
            .ok_or_else(|| format!("keyframe {key_number} is missing <cel-tag>; {USAGE}"))?;

        let time_60hz = time_text
            .parse::<f32>()
            .map_err(|error| format!("invalid <time-60hz> for keyframe {key_number}: {error}"))?;
        keyframes.push((time_60hz, cel_tag));
    }

    if keyframes.is_empty() {
        return Err(format!("at least one keyframe is required; {USAGE}").into());
    }
    Ok(keyframes)
}

#[cfg(test)]
mod tests {
    use super::parse_keyframes;

    #[test]
    fn parses_each_keyframe_from_named_fields() {
        let keys = parse_keyframes(["0", "A", "60", "B"].into_iter().map(str::to_owned)).unwrap();

        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], (0.0, "A".to_owned()));
        assert_eq!(keys[1], (60.0, "B".to_owned()));
    }

    #[test]
    fn rejects_an_incomplete_keyframe_group() {
        let error = parse_keyframes(["0"].into_iter().map(str::to_owned)).unwrap_err();
        assert!(error.to_string().contains("missing <cel-tag>"));
    }
}
