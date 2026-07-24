//! Reads validated UTF-8 text and bounded opaque attributes from one text layer.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A layer ID keeps this example focused on the typed text-layer accessor.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features sqlite --example inspect_text -- <input.clip> <layer-id>",
    )?;
    let layer_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features sqlite --example inspect_text -- <input.clip> <layer-id>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features sqlite --example inspect_text -- <input.clip> <layer-id>"
                .into(),
        );
    }

    // Open the embedded SQLite database and request text data under default limits.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let Some(text_layer) = database.text_layer(layer_id, Limits::default())? else {
        return Err(format!("layer {layer_id} is not a supported text layer").into());
    };

    // Common layer-level values remain available alongside the decoded strings.
    println!(
        "layer {}: layer type {}, text type {}, attributes version {:?}, format version {:?}",
        text_layer.layer_id(),
        text_layer.layer_type(),
        text_layer.text_layer_type(),
        text_layer.attributes_version(),
        text_layer.version()
    );
    println!(
        "objects: {}; additional attribute bytes: {}",
        text_layer.objects().len(),
        text_layer.additional_attributes().map_or(0, <[u8]>::len)
    );

    // Text is UTF-8; per-object styling remains bounded opaque bytes for now.
    for (index, object) in text_layer.objects().iter().enumerate() {
        println!(
            "object {index}: text={:?}, attribute bytes={}",
            object.text(),
            object.attributes().len()
        );
    }
    Ok(())
}
