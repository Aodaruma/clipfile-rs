use std::{env, fs::File, process::ExitCode};

use clipfile::{ChunkKind, ClipFile, ExternalBody};

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let Some(path) = arguments.first() else {
        eprintln!(
            "usage: cargo run --example inspect -- <file.clip> \
             [--deep] [--database] [--raster] [--document] [--animation]"
        );
        return ExitCode::from(2);
    };
    let deep = arguments.iter().skip(1).any(|value| value == "--deep");
    let database = arguments.iter().skip(1).any(|value| value == "--database");
    let raster = arguments.iter().skip(1).any(|value| value == "--raster");
    let document = arguments.iter().skip(1).any(|value| value == "--document");
    let animation = arguments.iter().skip(1).any(|value| value == "--animation");

    match inspect(path, deep, database, raster, document, animation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(
    path: impl AsRef<std::path::Path>,
    deep: bool,
    database: bool,
    raster: bool,
    document: bool,
    animation: bool,
) -> clipfile::Result<()> {
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
    inspect_database_if_requested(&mut clip, database)?;
    inspect_raster_if_requested(&mut clip, raster)?;
    inspect_document_if_requested(&mut clip, document)?;
    inspect_animation_if_requested(&mut clip, animation)?;
    Ok(())
}

#[cfg(feature = "animation")]
fn inspect_animation_if_requested<R: std::io::Read + std::io::Seek>(
    clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        let database = clip.open_database()?;
        if let Some(animation) = clip.read_animation(&database, clip.limits())? {
            let keyframes = animation
                .tracks()
                .iter()
                .map(|track| track.keyframes().len())
                .sum::<usize>();
            let curves = animation
                .animation_tracks()
                .iter()
                .map(|track| track.curves().len())
                .sum::<usize>();
            let curve_keys = animation
                .animation_tracks()
                .iter()
                .flat_map(|track| track.curves())
                .map(|curve| curve.keyframes().len())
                .sum::<usize>();
            println!(
                "animation: timeline {}, {} fps, frames {}..={}, {} tracks, {} curves / {} keys, {} cel tracks / {} selected keys",
                animation.timeline().id(),
                animation.timeline().frame_rate(),
                animation.timeline().start_frame(),
                animation.timeline().end_frame(),
                animation.animation_tracks().len(),
                curves,
                curve_keys,
                animation.tracks().len(),
                keyframes,
            );
        } else {
            println!("animation: none");
        }
    }
    Ok(())
}

#[cfg(not(feature = "animation"))]
fn inspect_animation_if_requested<R: std::io::Read + std::io::Seek>(
    _clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        eprintln!("--animation requires: cargo run --features animation --example inspect -- ...");
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn inspect_document_if_requested<R: std::io::Read + std::io::Seek>(
    clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        let document = clip.read_document()?;
        println!(
            "document: version {}, {} canvas(es), {} layer row(s)",
            document.project().internal_version(),
            document.canvases().len(),
            document.layers().len()
        );
        for tree in document.layer_trees() {
            println!(
                "canvas {} tree: {} reachable, {} unreachable",
                tree.canvas_id(),
                tree.reachable_layer_count(),
                tree.unreachable_layer_ids().len()
            );
        }
    }
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn inspect_document_if_requested<R: std::io::Read + std::io::Seek>(
    _clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        eprintln!("--document requires: cargo run --features sqlite --example inspect -- ...");
    }
    Ok(())
}

#[cfg(feature = "raster")]
fn inspect_raster_if_requested<R: std::io::Read + std::io::Seek>(
    clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if !requested {
        return Ok(());
    }
    let database = clip.open_database()?;
    let attribute_blobs = {
        let mut statement = database
            .connection()
            .prepare("SELECT Attribute FROM Offscreen WHERE Attribute IS NOT NULL")?;
        statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for attributes in &attribute_blobs {
        clipfile::OffscreenAttributes::parse(attributes)?;
    }
    println!("validated offscreen attributes: {}", attribute_blobs.len());
    let layer_ids = {
        let mut statement = database.connection().prepare(
            "SELECT l.MainId FROM Layer AS l \
             JOIN Mipmap AS m ON m.MainId = l.LayerRenderMipmap \
             JOIN MipmapInfo AS mi ON mi.MainId = m.BaseMipmapInfo \
             JOIN Offscreen AS o ON o.MainId = mi.Offscreen \
             JOIN ExternalChunk AS e ON CAST(e.ExternalID AS BLOB) = CAST(o.BlockData AS BLOB) \
             ORDER BY l.MainId",
        )?;
        statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    println!("raster candidates: {}", layer_ids.len());
    let mut first_unsupported = None;
    for layer_id in layer_ids {
        let Some(source) = database.layer_raster_source(layer_id)? else {
            continue;
        };
        match clip.decode_raster(&database, &source) {
            Ok(image) => {
                println!(
                    "decoded layer {layer_id}: {}x{} {:?}, {} bytes ({:?})",
                    image.width(),
                    image.height(),
                    image.format(),
                    image.pixels().len(),
                    image.data_state()
                );
                return Ok(());
            }
            Err(error @ clipfile::Error::UnsupportedRaster { .. }) => {
                first_unsupported.get_or_insert(error.to_string());
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(reason) = first_unsupported {
        eprintln!("first unsupported raster: {reason}");
    }
    eprintln!("no supported raster layer with external block data was found");
    Ok(())
}

#[cfg(not(feature = "raster"))]
fn inspect_raster_if_requested<R: std::io::Read + std::io::Seek>(
    _clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        eprintln!("--raster requires: cargo run --features raster --example inspect -- ...");
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn inspect_database_if_requested<R: std::io::Read + std::io::Seek>(
    clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        let database = clip.open_database()?;
        database.quick_check()?;
        clip.validate_external_index(&database)?;
        println!("SQLite tables: {}", database.schema().tables().len());
        println!(
            "SQLite external rows: {}",
            database.row_count("ExternalChunk")?
        );
    }
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn inspect_database_if_requested<R: std::io::Read + std::io::Seek>(
    _clip: &mut ClipFile<R>,
    requested: bool,
) -> clipfile::Result<()> {
    if requested {
        eprintln!("--database requires: cargo run --features sqlite --example inspect -- ...");
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
