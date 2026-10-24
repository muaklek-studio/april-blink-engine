//! Help → About: the editor's version and build, the Rust toolchain, the graphics backend
//! and the machine it runs on, with a button to copy it all (for bug reports).
//!
//! What needs other programs (`rustc --version`, `sysctl`, PowerShell…) is read once, in
//! the background, the first time the dialog opens.

use crate::i18n::t;
use crate::theme;
use ab_engine::core::jobs::{self, Task};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

/// The machine and the installed toolchain (slow to read: other programs run).
#[derive(Debug, Clone, Default)]
pub struct Machine {
    pub os: String,
    pub cpu: String,
    pub cores: usize,
    /// Bytes of memory, when known.
    pub memory: Option<u64>,
    /// `rustc --version` and `cargo --version` on this machine (game scripts are built with
    /// them), when installed.
    pub rustc: Option<String>,
    pub cargo: Option<String>,
}

/// What the dialog shows: the editor's own facts, the GPU, and the machine once read.
#[derive(Default)]
pub struct About {
    gpu: Option<wgpu::AdapterInfo>,
    machine: Option<Machine>,
    reading: Option<Task<Machine>>,
}

/// `0.1.0 (ae7553aea)`: the version, and the commit it was built from when known.
fn version() -> String {
    match env!("AB_BUILD_COMMIT") {
        "" => env!("CARGO_PKG_VERSION").to_owned(),
        commit => format!("{} ({commit})", env!("CARGO_PKG_VERSION")),
    }
}

impl About {
    pub fn new(gpu: Option<wgpu::AdapterInfo>) -> Self {
        Self {
            gpu,
            ..Self::default()
        }
    }

    /// The facts, as (section, [(label, value)]), in the dialog's order.
    fn sections(&self) -> Vec<(String, Vec<(String, String)>)> {
        let unknown = || t("about.unknown");
        let reading = || t("about.reading");
        let program = vec![(
            t("about.build"),
            format!("{} · {}", env!("AB_BUILD_PROFILE"), env!("AB_BUILD_TARGET")),
        )];
        let machine = self.machine.as_ref();
        let installed = |value: Option<&Option<String>>| match value {
            None => reading(),
            Some(Some(version)) => version.clone(),
            Some(None) => t("about.not_installed"),
        };
        let rust = vec![
            (t("about.built_with"), env!("AB_BUILD_RUSTC").to_owned()),
            (t("about.rustc"), installed(machine.map(|m| &m.rustc))),
            (t("about.cargo"), installed(machine.map(|m| &m.cargo))),
            (
                t("about.needed"),
                format!("{} {}", env!("CARGO_PKG_RUST_VERSION"), t("about.or_newer")),
            ),
        ];
        let graphics = match &self.gpu {
            Some(gpu) => vec![
                (t("about.backend"), format!("{:?}", gpu.backend)),
                (t("about.gpu"), gpu.name.clone()),
                (t("about.gpu_type"), gpu_type(gpu.device_type)),
                (
                    t("about.driver"),
                    [gpu.driver.as_str(), gpu.driver_info.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(80)
                        .collect::<String>(),
                ),
            ],
            None => vec![(t("about.backend"), t("about.no_gpu"))],
        };
        // A backend that names no driver (Metal) leaves the row out.
        let graphics = graphics
            .into_iter()
            .filter(|(k, v)| !(v.is_empty() && *k == t("about.driver")))
            .map(|(k, v)| (k, if v.is_empty() { unknown() } else { v }))
            .collect();
        let system = match machine {
            None => vec![(t("about.os"), reading())],
            Some(m) => vec![
                (t("about.os"), m.os.clone()),
                (t("about.cpu"), m.cpu.clone()),
                (t("about.cores"), m.cores.to_string()),
                (
                    t("about.memory"),
                    m.memory
                        .map(|bytes| format!("{:.1} GB", bytes as f64 / 1024f64.powi(3)))
                        .unwrap_or_else(unknown),
                ),
            ],
        };
        vec![
            (t("about.program"), program),
            (t("about.rust"), rust),
            (t("about.graphics"), graphics),
            (t("about.machine"), system),
        ]
    }

    /// Everything as text, for the clipboard.
    fn report(&self) -> String {
        let mut text = format!("April Blink Editor {}\n", version());
        for (section, rows) in self.sections() {
            text.push_str(&format!("\n[{section}]\n"));
            for (label, value) in rows {
                text.push_str(&format!("{label}: {value}\n"));
            }
        }
        text
    }

    /// The dialog's body. Returns whether Close was pressed.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        if self.machine.is_none() {
            let task = self
                .reading
                .get_or_insert_with(|| jobs::spawn_background("ab-about", read_machine));
            match task.try_take() {
                Some(machine) => {
                    self.machine = Some(machine);
                    self.reading = None;
                }
                None => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        let mut close = false;
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(icon::LIGHTNING)
                    .size(36.0)
                    .color(theme::ACCENT),
            );
            ui.heading("April Blink Editor");
            ui.label(
                RichText::new(format!("{} {}", t("about.version"), version()))
                    .color(theme::TEXT_DIM),
            );
        });
        ui.add_space(8.0);
        for (index, (section, rows)) in self.sections().into_iter().enumerate() {
            ui.label(
                RichText::new(section)
                    .font(theme::bold(13.0))
                    .color(theme::ACCENT),
            );
            egui::Grid::new(("about", index))
                .num_columns(2)
                .spacing([16.0, 4.0])
                .min_col_width(130.0)
                .show(ui, |ui| {
                    for (label, value) in rows {
                        ui.label(RichText::new(label).color(theme::TEXT_DIM));
                        ui.add(egui::Label::new(value).wrap());
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  {}", icon::COPY, t("about.copy")))
                .on_hover_text(t("about.copy_hint"))
                .clicked()
            {
                ui.ctx().copy_text(self.report());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("dialog.close")).clicked() {
                    close = true;
                }
            });
        });
        close
    }
}

fn gpu_type(kind: wgpu::DeviceType) -> String {
    use wgpu::DeviceType;
    t(match kind {
        DeviceType::IntegratedGpu => "about.integrated",
        DeviceType::DiscreteGpu => "about.discrete",
        DeviceType::VirtualGpu => "about.virtual",
        DeviceType::Cpu => "about.software",
        DeviceType::Other => "about.unknown",
    })
}

/// The first line a program prints, when it runs.
fn first_line(program: &str, args: &[&str]) -> Option<String> {
    let output = ab_project::process::command(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_owned())
}

/// Reads the machine (runs other programs: call it in the background).
pub fn read_machine() -> Machine {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    Machine {
        os: os_name(),
        cpu: cpu_name().unwrap_or_else(|| std::env::consts::ARCH.to_owned()),
        cores: std::thread::available_parallelism().map_or(0, |n| n.get()),
        memory: memory_bytes(),
        rustc: first_line("rustc", &["--version"]),
        cargo: first_line(&cargo, &["--version"]),
    }
}

fn os_name() -> String {
    let arch = std::env::consts::ARCH;
    let name = if cfg!(target_os = "macos") {
        first_line("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}"))
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_owned())
            })
    } else if cfg!(windows) {
        first_line("cmd", &["/C", "ver"])
    } else {
        None
    };
    format!(
        "{} ({arch})",
        name.unwrap_or_else(|| std::env::consts::OS.to_owned())
    )
}

fn cpu_name() -> Option<String> {
    if cfg!(target_os = "macos") {
        first_line("sysctl", &["-n", "machdep.cpu.brand_string"])
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| v.trim().to_owned())
            })
    } else if cfg!(windows) {
        first_line(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_Processor | Select-Object -First 1).Name",
            ],
        )
    } else {
        None
    }
}

fn memory_bytes() -> Option<u64> {
    if cfg!(target_os = "macos") {
        first_line("sysctl", &["-n", "hw.memsize"])?.parse().ok()
    } else if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kib: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        Some(kib * 1024)
    } else if cfg!(windows) {
        first_line(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ],
        )?
        .parse()
        .ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_is_described() {
        let machine = read_machine();
        assert!(!machine.os.is_empty());
        assert!(!machine.cpu.is_empty());
        assert!(machine.cores > 0);
        let about = About {
            machine: Some(machine),
            ..About::default()
        };
        let report = about.report();
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(report.contains(env!("AB_BUILD_RUSTC")));
    }
}
