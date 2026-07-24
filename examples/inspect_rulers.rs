//! Discovers ruler layers and summarizes validated vector/special-ruler metadata.

use std::{env, fs::File, process::ExitCode};

use clipfile::{ClipFile, Limits};

fn main() -> ExitCode {
    // Accept one input path so the example has predictable CLI behavior.
    let mut arguments = env::args_os().skip(1);
    let Some(path) = arguments.next() else {
        eprintln!("usage: cargo run --features sqlite --example inspect_rulers -- <file.clip>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: cargo run --features sqlite --example inspect_rulers -- <file.clip>");
        return ExitCode::from(2);
    }

    // Report all validation or schema errors without panicking.
    match inspect(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: impl AsRef<std::path::Path>) -> clipfile::Result<()> {
    // Open the typed document database; its schema details stay inside clipfile.
    let mut clip = ClipFile::open(File::open(path)?)?;
    let database = clip.open_database()?;

    // Discover and validate vector/special rulers without writing SQL.
    let layers = database.ruler_layers(Limits::default())?;
    println!("ruler layers: {}", layers.len());
    for layer in layers {
        // RulerKind gives callers a stable summary without matching every variant.
        let kinds = layer
            .rulers()
            .iter()
            .map(|ruler| format!("{:?}", ruler.kind()))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "layer={}\tvector={:?}\tmanager={:?}\trulers={}\tkinds={}",
            layer.layer_id(),
            layer.vector_object_id(),
            layer.manager_id(),
            layer.rulers().len(),
            kinds
        );
    }
    Ok(())
}
