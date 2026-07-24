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
    // Ruler ownership and special-ruler tables live in the embedded database.
    let mut clip = ClipFile::open(File::open(path)?)?;
    let database = clip.open_database()?;
    if !database.schema().has_column("Layer", "RulerRange") {
        println!("ruler layers: 0");
        return Ok(());
    }

    // Discover candidate layer IDs before asking for their typed ruler data.
    let mut statement = database
        .connection()
        .prepare("SELECT MainId FROM Layer WHERE RulerRange IS NOT NULL ORDER BY MainId")?;
    let layer_ids = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    // ruler_layer validates manager chains, curve records, and perspective links.
    println!("ruler layers: {}", layer_ids.len());
    for layer_id in layer_ids {
        if let Some(layer) = database.ruler_layer(layer_id, Limits::default())? {
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
    }
    Ok(())
}
