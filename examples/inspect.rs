use std::{env, fs::File, process::ExitCode};

use clipfile::{ChunkKind, ClipFile, ExternalBody};

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: cargo run --example inspect -- <file.clip> [--deep]");
        return ExitCode::from(2);
    };
    let deep = env::args_os().nth(2).is_some_and(|value| value == "--deep");

    match inspect(path, deep) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: impl AsRef<std::path::Path>, deep: bool) -> clipfile::Result<()> {
    let mut clip = ClipFile::open(File::open(path)?)?;
    let root = clip.root_header();
    let header = clip.file_header();
    println!("file size: {}", root.declared_file_size());
    println!("format value: {}", header.format_version());
    println!("database offset: {}", header.database_offset());
    println!("file identifier: {}", hex(header.identifier()));

    let summary = clip.validate()?;
    println!("external chunks: {}", summary.external_chunks());
    println!("SQLite payload size: {}", summary.database_payload_size());
    if deep {
        inspect_external_objects(&mut clip)?;
    }
    Ok(())
}

fn inspect_external_objects<R: std::io::Read + std::io::Seek>(
    clip: &mut ClipFile<R>,
) -> clipfile::Result<()> {
    let chunks = clip.chunks().collect::<clipfile::Result<Vec<_>>>()?;
    let mut block_objects = 0_u64;
    let mut blocks = 0_u64;
    let mut present_blocks = 0_u64;
    let mut compressed_objects = 0_u64;
    let mut media_objects = 0_u64;
    let mut unknown_objects = 0_u64;
    for chunk in chunks
        .iter()
        .filter(|chunk| chunk.kind() == ChunkKind::External)
    {
        let object = clip.inspect_external_chunk(chunk)?;
        match object.body() {
            ExternalBody::BlockData => {
                let data = clip.read_block_data(&object)?;
                block_objects += 1;
                blocks += data.blocks().len() as u64;
                present_blocks += data.present_blocks() as u64;
            }
            ExternalBody::LengthPrefixedZlib(_) => compressed_objects += 1,
            ExternalBody::Media(_) => media_objects += 1,
            ExternalBody::Unknown => unknown_objects += 1,
            _ => unknown_objects += 1,
        }
    }
    println!("block-data objects: {block_objects}");
    println!("blocks: {blocks} ({present_blocks} with data)");
    println!("length-prefixed zlib objects: {compressed_objects}");
    println!("media objects: {media_objects}");
    println!("unknown external objects: {unknown_objects}");
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
