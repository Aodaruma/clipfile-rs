use std::{env, fs::File};

use clipfile::{BlockChecksumMode, ClipFile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input = arguments
        .next()
        .ok_or("usage: invert_first_tile <input.clip> <new-output.clip> <layer-id>")?;
    let output = arguments
        .next()
        .ok_or("usage: invert_first_tile <input.clip> <new-output.clip> <layer-id>")?;
    let layer_id = arguments
        .next()
        .ok_or("usage: invert_first_tile <input.clip> <new-output.clip> <layer-id>")?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err("usage: invert_first_tile <input.clip> <new-output.clip> <layer-id>".into());
    }

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
    for byte in &mut bytes {
        *byte = u8::MAX - *byte;
    }
    drop(database);

    let mut writer = clip.writer()?;
    let block_summary =
        writer.replace_block_bytes(&identifier, block_index, bytes, BlockChecksumMode::Zero)?;
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
