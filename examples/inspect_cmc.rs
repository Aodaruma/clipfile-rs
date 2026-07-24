use std::{env, process::ExitCode};

use clipfile::{CmcFile, Limits};

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: cargo run --features sqlite --example inspect_cmc -- <file.cmc>");
        return ExitCode::from(2);
    };

    match CmcFile::open(path, Limits::default()) {
        Ok(cmc) => {
            println!("version: {}", cmc.internal_version());
            println!("nodes: {}", cmc.nodes().len());
            println!("pages: {}", cmc.page_nodes().count());
            for node in cmc.page_nodes() {
                println!(
                    "node {}: {}",
                    node.id(),
                    node.page_file_name().unwrap_or("<unknown link form>")
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
