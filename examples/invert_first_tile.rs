//! Re-encodes one native raster tile and writes a new `.clip` file.
//!
//! This intentionally inverts every native byte, including alpha. It demonstrates
//! the low-level block writer rather than a color-aware image editing operation.

use std::{env, fs::File};

use clipfile::{BlockChecksumMode, ClipFile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Select one layer and a new output path; the writer never overwrites a file.
    let mut arguments = env::args_os().skip(1);
    let input = arguments
        .next()
        .ok_or("usage: cargo run --features \"write,raster\" --example invert_first_tile -- <input.clip> <new-output.clip> <layer-id>")?;
    let output = arguments
        .next()
        .ok_or("usage: cargo run --features \"write,raster\" --example invert_first_tile -- <input.clip> <new-output.clip> <layer-id>")?;
    let layer_id = arguments
        .next()
        .ok_or("usage: cargo run --features \"write,raster\" --example invert_first_tile -- <input.clip> <new-output.clip> <layer-id>")?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err("usage: cargo run --features \"write,raster\" --example invert_first_tile -- <input.clip> <new-output.clip> <layer-id>".into());
    }

    // Resolve the selected layer through its render mipmap to external BlockData.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let source = database
        .layer_raster_source(layer_id)?
        .ok_or("the layer has no render raster")?;
    let identifier = source
        .external_identifier()
        .ok_or("the raster has no external block data")?
        .to_vec();
    let object = clip
        .resolve_external_object(&database, &identifier)?
        .ok_or("the external block-data object is absent")?;
    let blocks = clip.read_block_data(&object)?;

    // Decode only the first populated tile, keeping the example memory-bounded.
    let block = blocks
        .blocks()
        .iter()
        .find(|block| block.payload().is_some())
        .ok_or("the raster has no populated block")?;
    let block_index = block.index();
    let mut bytes = clip
        .decode_tile(block)?
        .ok_or("the selected block is empty")?
        .into_bytes();

    // DecodedTile contains the format's native planar channel arrangement.
    for byte in &mut bytes {
        *byte = u8::MAX - *byte;
    }
    drop(database);

    // Re-encode that block; checksum zero is an explicit compatibility opt-in.
    let mut writer = clip.writer()?;
    let block_summary =
        writer.replace_block_bytes(&identifier, block_index, bytes, BlockChecksumMode::Zero)?;

    // The complete container is rebuilt, offset-repaired, and validated at a new path.
    let write_summary = writer.write_to_path(output)?;
    println!(
        "block {}: {} decoded bytes -> {} compressed bytes; output {} bytes",
        block_summary.block_index(),
        block_summary.decoded_size(),
        block_summary.compressed_size(),
        write_summary.output_file_size()
    );
    Ok(())
}
