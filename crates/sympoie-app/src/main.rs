#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod agent;
mod agent_docs;
mod app;
mod audio;
mod document;
mod mcp;
mod plugins;
mod settings;
mod state;
mod ui;
mod views;

#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use raw_window_handle::HasWindowHandle;

use app::{Startup, SympoieApp};

const USAGE: &str = "usage: sympoie [PROJECT] [AUDIO.wav ...]\n       sympoie --mcp [PROJECT_FOLDER]   (MCP server for coding agents)";

fn parse_args(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Startup, String> {
    let mut startup = Startup {
        use_audio_device: std::env::var_os("SYMPOIE_NO_AUDIO").is_none(),
        ..Startup::default()
    };
    for argument in args {
        if argument == "--help" || argument == "-h" {
            return Err(USAGE.to_owned());
        }
        let path = PathBuf::from(argument);
        if sympoie::project_json::is_project_path(&path) {
            if startup.project.replace(path).is_some() {
                return Err(format!("only one project can be opened\n{USAGE}"));
            }
        } else {
            startup.audio.push(path);
        }
    }
    Ok(startup)
}

fn icon() -> egui::IconData {
    ui::brand::icon_data()
}

fn renderer() -> eframe::Renderer {
    #[cfg(feature = "glow")]
    if std::env::var("SYMPOIE_RENDERER").is_ok_and(|name| name.eq_ignore_ascii_case("glow")) {
        return eframe::Renderer::Glow;
    }
    eframe::Renderer::Wgpu
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--mcp") {
        let folder = mcp::bridge::project_folder(args.get(1).and_then(|arg| arg.to_str()));
        let stdin = std::io::stdin();
        mcp::bridge::run(folder, stdin.lock(), std::io::stdout().lock());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--probe-vst3") {
        if args.len() != 3 {
            eprintln!("usage: --probe-vst3 PLUGIN OUTPUT_JSON");
            std::process::exit(2);
        }
        match plugins::run_probe(
            std::path::Path::new(&args[1]),
            std::path::Path::new(&args[2]),
        ) {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }
    let mut startup = match parse_args(args.into_iter()) {
        Ok(startup) => startup,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    match settings::user_settings_path() {
        Ok(path) => {
            let (settings, error) = settings::load(&path);
            if let Some(error) = error {
                eprintln!("{error}");
            }
            startup.settings = settings;
            startup.settings_path = Some(path);
        }
        Err(error) => eprintln!("{error}"),
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sympoie")
            .with_app_id("sympoie")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(Arc::new(icon())),
        renderer: renderer(),
        ..Default::default()
    };
    eframe::run_native(
        "Sympoie",
        options,
        Box::new(move |cc| {
            let mut app = SympoieApp::new(&cc.egui_ctx, startup);
            app.load_library_folders();
            if let Ok(handle) = cc.window_handle()
                && let Ok(owner) = vst3_host::PluginWindowOwner::from_window_handle(handle)
            {
                app.set_plugin_window_owner(owner);
            }
            if app.scans_plugins_at_startup() {
                app.start_plugin_scan();
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_split_into_a_project_and_audio_files() {
        let args = ["song.symp", "drums.wav", "bass.WAV"].map(std::ffi::OsString::from);
        let startup = parse_args(args.into_iter()).unwrap();
        assert_eq!(startup.project, Some(PathBuf::from("song.symp")));
        assert_eq!(startup.audio.len(), 2);
        let twice = ["a.symp", "b.SYMP"].map(std::ffi::OsString::from);
        assert!(parse_args(twice.into_iter()).is_err());
    }

    #[test]
    fn icon_is_square_rgba() {
        let icon = icon();
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
