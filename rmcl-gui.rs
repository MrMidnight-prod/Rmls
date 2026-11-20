#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! GUI front-end: runs the `rmcl` CLI (same folder) in the background and streams its output.

use eframe::egui;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

enum Msg {
    Log(String),
    Version(String),
    Done,
}

struct App {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    child: Arc<Mutex<Option<Child>>>,
    log: Vec<String>,
    versions: Vec<String>,
    version: String,
    name: String,
    ram_mb: u32,
    tier: usize, // 0 = lowend, 1 = normal
    fabric: bool,
    perf: bool,
    server: String,
    java: String,
}

fn cli_path() -> PathBuf {
    let exe = if cfg!(windows) { "rmcl.exe" } else { "rmcl" };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe)))
        .unwrap_or_else(|| PathBuf::from(exe))
}

fn base_cmd<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Command {
    let mut c = Command::new(cli_path());
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn pump<R: Read + Send + 'static>(r: R, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut rd = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match rd.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf).trim_end().to_string();
                    let _ = tx.send(Msg::Log(s));
                    ctx.request_repaint();
                }
            }
        }
    });
}

impl App {
    fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let (tx2, ctx2) = (tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            match base_cmd(&["list", "--limit", "80"]).output() {
                Ok(out) => {
                    for l in String::from_utf8_lossy(&out.stdout).lines() {
                        if l.starts_with("latest") {
                            continue;
                        }
                        if let Some(v) = l.split_whitespace().next() {
                            let _ = tx2.send(Msg::Version(v.to_string()));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx2.send(Msg::Log(format!("cannot run {}: {e}", cli_path().display())));
                }
            }
            ctx2.request_repaint();
        });
        Self {
            ctx,
            tx,
            rx,
            child: Arc::new(Mutex::new(None)),
            log: vec!["Ready. Pick a version and press Play.".into()],
            versions: vec!["latest".into()],
            version: "1.20.1".into(),
            name: "Player".into(),
            ram_mb: 2560,
            tier: 0,
            fabric: true,
            perf: true,
            server: String::new(),
            java: "java".into(),
        }
    }

    fn busy(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    fn common_args(&self) -> Vec<String> {
        let mut a = vec![self.version.clone()];
        if self.fabric {
            a.push("--fabric".into());
            if self.perf {
                a.push("--perf-mods".into());
            }
        }
        a.push("--tier".into());
        a.push(if self.tier == 0 { "lowend" } else { "normal" }.into());
        a
    }

    fn run_cli(&mut self, args: Vec<String>) {
        self.log.push(format!("$ rmcl {}", args.join(" ")));
        let mut cmd = base_cmd(&args);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.log.push(format!("cannot start {}: {e}", cli_path().display()));
                return;
            }
        };
        pump(child.stdout.take().unwrap(), self.tx.clone(), self.ctx.clone());
        pump(child.stderr.take().unwrap(), self.tx.clone(), self.ctx.clone());
        *self.child.lock().unwrap() = Some(child);

        let (slot, tx, ctx) = (self.child.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            let mut g = slot.lock().unwrap();
            if let Some(c) = g.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    *g = None;
                    let _ = tx.send(Msg::Log(format!("[finished: {st}]")));
                    let _ = tx.send(Msg::Done);
                    ctx.request_repaint();
                    break;
                }
            } else {
                break;
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(l) => self.log.push(l),
                Msg::Version(v) => self.versions.push(v),
                Msg::Done => {}
            }
        }
        if self.log.len() > 5000 {
            self.log.drain(..self.log.len() - 5000);
        }
        let busy = self.busy();

        egui::SidePanel::left("opts").min_width(270.0).show(ctx, |ui| {
            ui.heading("rmcl launcher");
            ui.separator();
            ui.add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_label("Version")
                    .selected_text(self.version.clone())
                    .show_ui(ui, |ui| {
                        for v in &self.versions {
                            ui.selectable_value(&mut self.version, v.clone(), v);
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("Player");
                    ui.text_edit_singleline(&mut self.name);
                });
                ui.add(egui::Slider::new(&mut self.ram_mb, 1024..=6144).step_by(256.0).text("RAM MB"));
                ui.label("Hardware profile");
                ui.radio_value(&mut self.tier, 0, "Low-end (2 cores, 4-6 GB RAM)");
                ui.radio_value(&mut self.tier, 1, "Normal (multi-core)");
                ui.checkbox(&mut self.fabric, "Fabric loader");
                ui.add_enabled(self.fabric, egui::Checkbox::new(&mut self.perf, "Install performance mods"));
                ui.horizontal(|ui| {
                    ui.label("Server");
                    ui.add(egui::TextEdit::singleline(&mut self.server).hint_text("empty = singleplayer"));
                });
                ui.horizontal(|ui| {
                    ui.label("Java");
                    ui.add(egui::TextEdit::singleline(&mut self.java).hint_text("java = auto-download"));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Install only")).clicked() {
                    let mut a = vec!["install".to_string()];
                    a.extend(self.common_args());
                    self.run_cli(a);
                }
                if ui.add_enabled(!busy, egui::Button::new("  Play  ")).clicked() {
                    let mut a = vec!["launch".to_string()];
                    a.extend(self.common_args());
                    a.extend(["--name".into(), self.name.clone(), "--ram".into(), format!("{}M", self.ram_mb)]);
                    a.extend(["--java".into(), self.java.clone()]);
                    if !self.server.trim().is_empty() {
                        a.extend(["--server".into(), self.server.trim().to_string()]);
                    }
                    self.run_cli(a);
                }
                if ui.add_enabled(busy, egui::Button::new("Cancel")).clicked() {
                    if let Some(c) = self.child.lock().unwrap().as_mut() {
                        let _ = c.kill(); // stops the installer; a running game is closed from its own window
                    }
                }
            });
            if busy {
                ui.add(egui::Spinner::new());
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for l in &self.log {
                    ui.monospace(l);
                }
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([860.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("rmcl", opts, Box::new(|cc| Box::new(App::new(cc.egui_ctx.clone()))))
}
