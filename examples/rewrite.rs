use std::{env, fs::File};

use clipfile::ClipFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input = arguments
        .next()
        .ok_or("usage: rewrite <input.clip> <new-output.clip>")?;
    let output = arguments
        .next()
        .ok_or("usage: rewrite <input.clip> <new-output.clip>")?;
    if arguments.next().is_some() {
        return Err("usage: rewrite <input.clip> <new-output.clip>".into());
    }

    let mut clip = ClipFile::open(File::open(input)?)?;
    let mut writer = clip.writer()?;
    let summary = writer.write_to_path(output)?;

    println!(
        "wrote {} bytes ({} external chunks, {} replacements)",
        summary.output_file_size(),
        summary.external_chunks(),
        summary.replaced_external_bodies()
    );
    Ok(())
}
