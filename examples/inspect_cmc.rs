//! Opens a standalone page-management `.cmc` file and lists its validated pages.

use std::{env, process::ExitCode};

use clipfile::{CmcFile, Limits};

fn main() -> ExitCode {
    // Accept one path and reject extra arguments instead of silently ignoring them.
    let mut arguments = env::args_os().skip(1);
    let Some(path) = arguments.next() else {
        eprintln!("usage: cargo run --features sqlite --example inspect_cmc -- <file.cmc>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: cargo run --features sqlite --example inspect_cmc -- <file.cmc>");
        return ExitCode::from(2);
    }

    // CmcFile validates the standalone SQLite schema and complete node tree.
    match CmcFile::open(path, Limits::default()) {
        Ok(cmc) => {
            println!("version: {}", cmc.internal_version());
            println!("nodes: {}", cmc.nodes().len());
            println!("pages: {}", cmc.page_nodes().count());

            // Safe observed link forms can also be resolved relative to the CMC file.
            for node in cmc.page_nodes() {
                println!(
                    "node {}: {}; resolved path: {:?}",
                    node.id(),
                    node.page_file_name().unwrap_or("<unknown link form>"),
                    cmc.page_path(node.id())
                );
            }
            ExitCode::SUCCESS
        }
        // Parsing, limits, schema, and tree errors share the crate's typed error.
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
