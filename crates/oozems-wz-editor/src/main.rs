#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use app::EditorApp;
use app::EditorPaths;
use app::PreparedEditor;
use eframe::egui;

mod app;
mod document;
mod save;
mod scripts;

fn main() {
    if let Err(error) = run() {
        eprintln!("oozems-wz-editor: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let Some(paths) = parse_args(std::env::args().skip(1))? else {
        print_help();
        return Ok(());
    };
    let prepared = PreparedEditor::load(paths)?;
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1_360.0, 860.0])
            .with_min_inner_size([960.0, 620.0]),
        renderer: eframe::Renderer::Glow,
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Oozems WZ Studio",
        native_options,
        Box::new(move |context| Ok(Box::new(EditorApp::new(context, prepared)))),
    )?;
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Option<EditorPaths>, String> {
    let mut data = PathBuf::from("data");
    let mut quest = None;
    let mut skill = None;
    let mut strings = None;
    let mut scripts = None;
    let mut output_directory = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if matches!(argument.as_str(), "-h" | "--help") {
            return Ok(None);
        }
        if !matches!(
            argument.as_str(),
            "--data" | "--quest" | "--skill" | "--strings" | "--scripts" | "--output-directory"
        ) {
            return Err(format!("unknown argument {argument:?}"));
        }
        let value = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| format!("{argument} requires a path"))?;
        match argument.as_str() {
            "--data" => data = value,
            "--quest" => quest = Some(value),
            "--skill" => skill = Some(value),
            "--strings" => strings = Some(value),
            "--scripts" => scripts = Some(value),
            "--output-directory" => output_directory = Some(value),
            _ => unreachable!("arguments were validated above"),
        }
    }
    let output = output_directory.unwrap_or_else(|| data.clone());
    Ok(Some(EditorPaths {
        quest: quest.unwrap_or_else(|| data.join("Quest.wz")),
        quest_output: output.join("Quest.edited.wz"),
        skill: skill.unwrap_or_else(|| data.join("Skill.wz")),
        skill_output: output.join("Skill.edited.wz"),
        strings: strings.unwrap_or_else(|| data.join("String.wz")),
        scripts: scripts.unwrap_or_else(|| data.join("quest-scripts.toml")),
    }))
}

fn print_help() {
    println!(
        "Oozems WYSIWYG quest and skill editor\n\nUsage: cargo run --package oozems-wz-editor -- \
         [OPTIONS]\n\nOptions:\n--data <DIR>              WZ data directory [default: \
         data]\n--quest <PATH>            Quest.wz path\n--skill <PATH>            Skill.wz \
         path\n--strings <PATH>          String.wz path\n--scripts <PATH>          Quest script \
         TOML path\n--output-directory <DIR>  Edited WZ output directory [default: data]\n-h, \
         --help               Show this help"
    );
}
