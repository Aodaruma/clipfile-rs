use std::{env, fs::File, process::ExitCode};

use clipfile::ClipFile;

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: cargo run --example inspect -- <file.clip>");
        return ExitCode::from(2);
    };

    match inspect(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: impl AsRef<std::path::Path>) -> clipfile::Result<()> {
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
