//! Performs a validated no-change rewrite into a new `.clip` file.

use std::{env, fs::File};

use clipfile::ClipFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Require separate input/output paths; write_to_path refuses existing outputs.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features write --example rewrite -- <input.clip> <new-output.clip>",
    )?;
    let output = arguments.next().ok_or(
        "usage: cargo run --features write --example rewrite -- <input.clip> <new-output.clip>",
    )?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features write --example rewrite -- <input.clip> <new-output.clip>"
                .into(),
        );
    }

    // writer() strictly validates the source and clones its SQLite database.
    let mut clip = ClipFile::open(File::open(input)?)?;
    let mut writer = clip.writer()?;

    // With no edits queued, the result should be byte-exact for supported layouts.
    let summary = writer.write_to_path(output)?;

    // WriteSummary reports the rebuilt container rather than semantic document data.
    println!(
        "wrote {} bytes ({} external chunks, {} replacements)",
        summary.output_file_size(),
        summary.external_chunks(),
        summary.replaced_external_bodies()
    );
    Ok(())
}
