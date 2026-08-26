#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use app::EditorApp;
use app::PreparedEditor;
use eframe::egui;

mod app;
mod wz;

fn main() {
    if let Err(error) = run() {
        eprintln!("oozems-ui-editor: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_args(std::env::args().skip(1))?;
    let Some(options) = options else {
        print_help();
        return Ok(());
    };
    let prepared = PreparedEditor::load(options.wz_path, options.layout_directory)?;
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1_280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Glow,
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Oozems UI Layout Editor",
        native_options,
        Box::new(move |context| Ok(Box::new(EditorApp::new(context, prepared)))),
    )?;
    Ok(())
}

struct EditorOptions {
    wz_path: PathBuf,
    layout_directory: PathBuf,
}

fn parse_args(
    arguments: impl IntoIterator<Item = String>
) -> Result<Option<EditorOptions>, String> {
    let mut wz_path = PathBuf::from("data/UI.wz");
    let mut layout_directory = PathBuf::from("config/gui");
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--wz" => {
                wz_path = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--wz requires a path")?;
            }
            "--layouts" => {
                layout_directory = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--layouts requires a directory")?;
            }
            "-h" | "--help" => return Ok(None),
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    Ok(Some(EditorOptions {
        wz_path,
        layout_directory,
    }))
}

fn print_help() {
    println!(
        "Oozems local WYSIWYG UI layout editor\n\nUsage: cargo run --package oozems-ui-editor -- \
         [OPTIONS]\n\nOptions:\n--wz <PATH>          UI.wz path [default: data/UI.wz]\n--layouts \
         <DIR>      Textproto directory [default: config/gui]\n-h, --help           Show this help"
    );
}
