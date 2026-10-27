//! April Blink Editor.
//!
//! Starts with a loading screen (system check), then the project manager; the editor
//! opens on the project chosen there. `--project` opens a project folder (building its
//! scripts), or an asset folder directly (with `--scripts` for its library).
//!
//! ```text
//! cargo run -p ab-editor                                       # project manager
//! cargo run -p ab-editor -- --new-project "My Game" --in ~/Games    # no window
//! cargo run -p ab-editor -- --export <project folder> --platform all --archive
//! cargo run -p ab-editor -- --project assets --scene slice/level.abscene \
//!     --scripts target/debug/libab_slice.so                      # with the game's scripts
//! ```

// Release builds are windowed programs on Windows (no console window next to the editor).
// Started from a terminal, they print to it (see `attach_console`); an error that stops the
// editor before its window opens is shown in a message box otherwise.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Counts the heap for the memory profiler (see `ab_engine::memory`).
#[global_allocator]
static ALLOCATOR: ab_engine::core::memory::CountingAllocator =
    ab_engine::core::memory::CountingAllocator;

mod about;
mod app;
mod asset_sync;
mod asset_viewer;
mod audio_widgets;
mod build;
mod chrome;
mod commands;
mod console;
mod create;
mod dialogs;
mod diff_view;
mod export;
mod features;
mod fonts;
mod git;
mod git_window;
mod history;
mod i18n;
mod loading;
mod palette;
mod panels;
mod preferences;
mod preview;
mod project;
mod project_manager;
mod projects;
mod scripts;
mod settings;
mod shell;
mod state;
mod theme;
mod thumbnails;
mod tool_window;
mod ui_designer;
mod ui_edit;
mod viewport;
mod viewports;

use anyhow::{bail, Context as _};
use std::path::PathBuf;

/// Command-line options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// A project folder, or an asset directory to edit, skipping the project manager.
    pub project: Option<PathBuf>,
    /// Scene to open, relative to the project.
    pub scene: Option<String>,
    /// The game's script library (cdylib), for Play mode.
    pub scripts: Option<PathBuf>,
    /// Nodes to select at start (paths from the scene root, comma-separated).
    pub select: Option<String>,
    /// Start in Play mode.
    pub play: bool,
    /// Panels to bring to the front (`profiler,debug`).
    pub show: Vec<String>,
    /// Start with the physics debug view on.
    pub physics_view: bool,
    /// Stay on the loading screen (screenshots of it).
    pub hold_loading: bool,
    /// Use the system's title bar instead of the editor's own.
    pub system_titlebar: bool,
    /// Save a PNG of the window after start-up and exit (documentation, CI).
    pub screenshot: Option<PathBuf>,
    /// With `--screenshot`: tool windows are drawn inside the main window, so the grab shows
    /// them (pair with `--show window:<tool>`).
    pub screenshot_window: Option<String>,
    /// The editor's language for this run (`en`, `th`), over the saved preference.
    pub language: Option<String>,
    /// The asset folder the browser starts in (screenshots).
    pub browse: Option<String>,
    /// An asset to open in the viewer at start (screenshots).
    pub view: Option<String>,
}

/// The editor draws with the engine's GPU context: the same instance flags (no Vulkan
/// validation layer unless `WGPU_VALIDATION=1`), limits and error handling as the game.
/// Falls back to eframe's own setup when that cannot open a device.
fn wgpu_setup() -> eframe::egui_wgpu::WgpuSetup {
    use ab_engine::rendering::GpuContext;
    use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew, WgpuSetupExisting};
    match GpuContext::new_headless() {
        Ok(GpuContext {
            instance: Some(instance),
            adapter,
            device,
            queue,
            ..
        }) => WgpuSetup::Existing(WgpuSetupExisting {
            instance,
            adapter,
            device,
            queue,
        }),
        result => {
            if let Err(err) = result {
                tracing::warn!("using the default GPU setup: {err}");
            }
            let mut setup = WgpuSetupCreateNew::without_display_handle();
            setup.instance_descriptor.backends =
                wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL);
            setup.power_preference = wgpu::PowerPreference::HighPerformance;
            // The engine's renderer needs the same limits as in the game.
            setup.device_descriptor = std::sync::Arc::new(|adapter| wgpu::DeviceDescriptor {
                label: Some("ab-editor device"),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            });
            WgpuSetup::CreateNew(setup)
        }
    }
}

/// `ab-editor --export <slice|project folder> [options]`: exports without a window (what the
/// Export window runs in the background; also usable from a terminal or CI).
fn export(args: &[String]) -> anyhow::Result<()> {
    use ab_project::export;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (name, options) = export::parse_args(&args, std::path::Path::new("dist"))?;
    let game = export::resolve_game(&name)?;
    let exported = export::export(&game, &options, &mut |line: &str| println!("{line}"))?;
    println!();
    for done in exported {
        println!("{:<8} {}", done.platform.name(), done.folder.display());
        if let Some(archive) = done.archive {
            println!("{:<8} {}", "", archive.display());
        }
    }
    Ok(())
}

/// `ab-editor --sync-assets [<project folder>]`: brings the project's packed assets (in
/// git) and its files in step, as the editor does when it opens the project (after a
/// clone, on a build machine…). Prints what it did.
fn sync_assets(args: &[String]) -> anyhow::Result<()> {
    let folder = match args {
        [] => std::env::current_dir()?,
        [folder] => std::path::absolute(folder)?,
        _ => bail!("usage: ab-editor --sync-assets [<project folder>]"),
    };
    let project = ab_project::Project::open(&folder)?;
    ab_project::asset_store::ensure_git_files(&project)?;
    let report = ab_project::asset_store::sync(&project)?;
    for (what, paths) in [
        ("unpacked", &report.unpacked),
        ("removed", &report.removed),
        ("packed", &report.packed),
        ("conflict", &report.conflicts),
    ] {
        for path in paths {
            println!("{what:<9} {path}");
        }
    }
    println!("{}", report.summary());
    if project.lfs && !ab_project::asset_store::lfs_available() {
        println!(
            "Warning: the packed assets go to Git LFS, which is not installed: {}",
            ab_project::asset_store::LFS_INSTALL
        );
    }
    if !report.conflicts.is_empty() {
        println!(
            "Conflicts kept your files; the versions from git are in {}",
            project.data_dir().join("conflicts").display()
        );
    }
    Ok(())
}

/// `ab-editor --new-project "<title>" [--in <folder>]`: the project manager's New Project
/// without a window (`<folder>/<name>`; default: the current folder).
fn new_project(args: &[String]) -> anyhow::Result<()> {
    const USAGE: &str = "usage: ab-editor --new-project \"<title>\" [--in <folder>]";
    let (title, location) = match args {
        [title] => (title, std::env::current_dir()?),
        [title, flag, folder] if flag == "--in" => (title, std::path::absolute(folder)?),
        _ => bail!(USAGE),
    };
    std::fs::create_dir_all(&location)?;
    let project = ab_project::create(&ab_project::NewProject {
        title: title.clone(),
        location,
        new_folder: true,
    })?;
    println!("{}", project.root.display());
    Ok(())
}

fn parse() -> anyhow::Result<Options> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--project" => options.project = Some(value()?.into()),
            "--scene" => options.scene = Some(value()?),
            "--scripts" => options.scripts = Some(value()?.into()),
            "--play" => options.play = true,
            "--physics-view" => options.physics_view = true,
            "--system-titlebar" => options.system_titlebar = true,
            "--hold-loading" => options.hold_loading = true,
            "--show" => {
                options.show = value()?
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .collect();
            }
            "--select" => options.select = Some(value()?),
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--screenshot-window" => options.screenshot_window = Some(value()?),
            "--language" => options.language = Some(value()?),
            "--browse" => options.browse = Some(value()?),
            "--view" => options.view = Some(value()?),
            "-h" | "--help" => {
                println!(
                    "usage: ab-editor [--project <asset dir>] [--scene <scene.toml>] \
                     [--scripts <library>] [--select <node path,...>] [--play] [--physics-view] \
                     [--show <panel|window:<tool>|sound_event:<asset>|palette[:<query>],...>] [--system-titlebar] [--screenshot <file.png>] [--screenshot-window <tool>] \
                     [--language <en|th|…>] \
                     [--hold-loading]\n\n\
                     Without --project the project manager opens after the system check."
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument `{other}`"),
        }
    }
    Ok(options)
}

fn main() -> anyhow::Result<()> {
    let console = attach_console();
    let result = run();
    if let Err(err) = &result {
        if !console {
            ab_engine::crash::show_text("April Blink Editor", &format!("{err:#}"));
        }
    }
    result
}

/// Whether the editor's output reaches someone: always outside Windows. A Windows release
/// build has no console of its own, so it takes the one of the terminal it was started from
/// (`--export`, `--help`…), unless its output is already piped (the Export window's job).
#[cfg(windows)]
fn attach_console() -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn AttachConsole(process: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const INVALID_HANDLE_VALUE: *mut c_void = usize::MAX as *mut c_void;
    // SAFETY: plain Win32 calls with constant arguments; the handle is only compared.
    unsafe {
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        (!stdout.is_null() && stdout != INVALID_HANDLE_VALUE)
            || AttachConsole(ATTACH_PARENT_PROCESS) != 0
    }
}

#[cfg(not(windows))]
fn attach_console() -> bool {
    true
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--export") => return export(&args[1..]),
        Some("--new-project") => return new_project(&args[1..]),
        Some("--sync-assets") => return sync_assets(&args[1..]),
        _ => {}
    }
    let logs = console::install();
    let options = parse()?;
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("April Blink Editor")
            .with_decorations(options.system_titlebar)
            .with_app_id("april-blink-editor")
            // Not `with_resizable(false)`: that fixes the maximum size, and Wayland then
            // refuses the larger minimum size of the next screens.
            .with_inner_size(shell::LOADING_SIZE),
        centered: true,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: wgpu_setup(),
            ..Default::default()
        },
        depth_buffer: 0,
        ..Default::default()
    };
    eframe::run_native(
        "April Blink Editor",
        native,
        Box::new(move |cc| Ok(Box::new(shell::Shell::new(cc, options, logs)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
