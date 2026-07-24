use std::{env, fs::File, process::ExitCode};

use clipfile::{ClipFile, Limits};

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: inspect_rulers <file.clip>");
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
    if !database.schema().has_column("Layer", "RulerRange") {
        println!("ruler layers: 0");
        return Ok(());
    }

    let mut statement = database
        .connection()
        .prepare("SELECT MainId FROM Layer WHERE RulerRange IS NOT NULL ORDER BY MainId")?;
    let layer_ids = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    println!("ruler layers: {}", layer_ids.len());
    for layer_id in layer_ids {
        if let Some(layer) = database.ruler_layer(layer_id, Limits::default())? {
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
