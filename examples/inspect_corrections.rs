//! Discovers correction layers and prints their typed, validated parameters.

use std::{env, fs::File, process::ExitCode};

use clipfile::{ClipFile, Correction, Limits};

fn main() -> ExitCode {
    // Accept one input path and keep CLI failures distinct from parse failures.
    let mut arguments = env::args_os().skip(1);
    let Some(path) = arguments.next() else {
        eprintln!(
            "usage: cargo run --features sqlite --example inspect_corrections -- <file.clip>"
        );
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!(
            "usage: cargo run --features sqlite --example inspect_corrections -- <file.clip>"
        );
        return ExitCode::from(2);
    }

    // Convert the library Result into a conventional process exit code.
    match inspect(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: impl AsRef<std::path::Path>) -> clipfile::Result<()> {
    // Correction metadata lives in the embedded SQLite database.
    let mut clip = ClipFile::open(File::open(path)?)?;
    let database = clip.open_database()?;

    // Discover candidates and decode them without schema checks or SQL.
    let layers = database.correction_layers(Limits::default())?;
    println!("correction layers: {}", layers.len());
    for layer in layers {
        println!(
            "layer={}\ttype={}\tcorrection={}\tbytes={}",
            layer.layer_id(),
            layer.layer_type(),
            correction_summary(layer.correction()),
            layer.raw_attributes().len()
        );
    }
    Ok(())
}

fn correction_summary(correction: &Correction) -> String {
    // Keep the output compact while demonstrating all currently typed variants.
    match correction {
        Correction::BrightnessContrast {
            brightness,
            contrast,
        } => format!("brightness/contrast: {brightness}/{contrast}"),
        Correction::Levels { channels } => format!(
            "levels: {} channels; RGB input {}..{}",
            channels.len(),
            channels[0].input_left_8bit(),
            channels[0].input_right_8bit()
        ),
        Correction::ToneCurve { channels } => format!(
            "tone curve: {} channels, {} RGB points",
            channels.len(),
            channels[0].points().len()
        ),
        Correction::HueSaturationLuminosity {
            hue,
            saturation,
            luminosity,
        } => format!("hue/saturation/luminosity: {hue}/{saturation}/{luminosity}"),
        Correction::ColorBalance {
            keep_luminosity,
            shadows,
            midtones,
            highlights,
        } => format!(
            "color balance: keep={keep_luminosity}, cyan={}/{}/{}",
            shadows.cyan(),
            midtones.cyan(),
            highlights.cyan()
        ),
        Correction::ReverseGradient => "reverse gradient".to_owned(),
        Correction::Posterization { levels } => format!("posterization: {levels} levels"),
        Correction::Threshold { level } => format!("threshold: {level}"),
        Correction::GradientMap { stops } => format!("gradient map: {} stops", stops.len()),
        Correction::Unknown { kind, payload } => {
            format!("unknown kind {kind}: {} payload bytes", payload.len())
        }
        _ => format!("future correction kind {}", correction.kind()),
    }
}
