use std::{env, fs::File, process::ExitCode};

use clipfile::{ClipFile, Limits};

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: inspect_corrections <file.clip>");
        return ExitCode::FAILURE;
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
    let database = clip.open_database()?;
    if !database.schema().has_column("Layer", "FilterLayerInfo") {
        println!("correction layers: 0");
        return Ok(());
    }

    let mut statement = database
        .connection()
        .prepare("SELECT MainId FROM Layer WHERE FilterLayerInfo IS NOT NULL ORDER BY MainId")?;
    let layer_ids = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    println!("correction layers: {}", layer_ids.len());
    for layer_id in layer_ids {
        if let Some(layer) = database.correction_layer(layer_id, Limits::default())? {
            println!(
                "layer={}\ttype={}\tcorrection={}\tbytes={}",
                layer.layer_id(),
                layer.layer_type(),
                layer.correction().kind(),
                layer.raw_attributes().len()
            );
        }
    }
    Ok(())
}
