//! The window: two folder pickers, two checkboxes, an Install button and a log.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui;

use crate::{gamefind, pipeline};

enum Msg {
    Line(String),
    Done(Result<(), String>),
}

struct App {
    game: String,
    sd: String,
    want_3dsx: bool,
    want_cia: bool,
    cia_missing: Vec<String>,
    log: Vec<String>,
    running: bool,
    result: Option<Result<(), String>>,
    rx: Option<Receiver<Msg>>,
}

impl App {
    fn new() -> App {
        let cia_missing = pipeline::missing(true)
            .into_iter()
            .filter(|m| !pipeline::missing(false).contains(m))
            .collect::<Vec<_>>();
        App {
            game: gamefind::find_game()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            sd: String::new(),
            want_3dsx: true,
            want_cia: false,
            cia_missing,
            log: Vec::new(),
            running: false,
            result: None,
            rx: None,
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let o = pipeline::Options {
            game: PathBuf::from(self.game.trim()),
            sd: PathBuf::from(self.sd.trim()),
            want_3dsx: self.want_3dsx,
            want_cia: self.want_cia,
        };
        let (tx, rx) = channel();
        let (tx_log, ctx_log) = (tx.clone(), ctx.clone());
        self.rx = Some(rx);
        self.log.clear();
        self.result = None;
        self.running = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let tx_line = std::sync::Mutex::new(tx_log);
            let r = pipeline::install(&o, &move |l: &str| {
                tx_line.lock().unwrap().send(Msg::Line(l.to_string())).ok();
                ctx_log.request_repaint();
            });
            tx.send(Msg::Done(r)).ok();
            ctx.request_repaint();
        });
    }
}

fn folder_row(ui: &mut egui::Ui, label: &str, value: &mut String, enabled: bool, hint: &str) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.add_enabled(
            enabled,
            egui::TextEdit::singleline(value)
                .desired_width(ui.available_width() - 90.0)
                .hint_text(hint),
        );
        if ui
            .add_enabled(enabled, egui::Button::new("Browse..."))
            .clicked()
        {
            let mut dialog = rfd::FileDialog::new();
            if std::path::Path::new(value.trim()).is_dir() {
                dialog = dialog.set_directory(value.trim());
            }
            if let Some(p) = dialog.pick_folder() {
                *value = p.display().to_string();
            }
        }
    });
    ui.add_space(6.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Line(l) => self.log.push(l),
                    Msg::Done(r) => {
                        self.running = false;
                        self.result = Some(r);
                    }
                }
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Kingdom: New Lands for 3DS");
            ui.label("You need your own copy of Kingdom: New Lands on this computer (Steam or GOG, Windows or Linux).");
            ui.add_space(8.0);
            let idle = !self.running;
            folder_row(
                ui,
                "1. Kingdom: New Lands folder (the one that contains Kingdom_Data)",
                &mut self.game,
                idle,
                "e.g. the folder in steamapps/common/Kingdom New Lands",
            );
            folder_row(
                ui,
                "2. SD card (the card's top-level folder or drive)",
                &mut self.sd,
                idle,
                "put the SD card in this computer, then Browse",
            );
            ui.label("3. What to install");
            ui.horizontal(|ui| {
                ui.add_enabled(idle, egui::Checkbox::new(&mut self.want_3dsx, ".3dsx (Homebrew Launcher)"));
                let cia_ok = self.cia_missing.is_empty();
                let cia = ui.add_enabled(idle && cia_ok, egui::Checkbox::new(&mut self.want_cia, ".cia (HOME Menu, install with FBI)"));
                if !cia_ok {
                    cia.on_disabled_hover_text(format!("not in this package: {}", self.cia_missing.join(", ")));
                    self.want_cia = false;
                }
            });
            ui.add_space(8.0);
            let ready = idle && !self.game.trim().is_empty() && !self.sd.trim().is_empty() && (self.want_3dsx || self.want_cia);
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Install")).clicked() {
                    self.start(ctx);
                }
                if self.running {
                    ui.spinner();
                    ui.label("Working... this takes a few minutes the first time. Don't remove the SD card.");
                }
            });
            match &self.result {
                Some(Ok(())) => {
                    ui.colored_label(egui::Color32::from_rgb(40, 160, 70), "Finished. See the notes below.");
                }
                Some(Err(e)) => {
                    ui.colored_label(egui::Color32::from_rgb(220, 60, 50), format!("Failed: {e}"));
                }
                None => {}
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for l in &self.log {
                    ui.monospace(l);
                }
            });
        });
    }
}

/// Opens the window with OpenGL, or Direct3D/Vulkan when OpenGL isn't there
/// (a virtual machine without 3D acceleration only has OpenGL 1.1).
pub fn run() -> Result<(), String> {
    let mut errors = Vec::new();
    for renderer in [eframe::Renderer::Glow, eframe::Renderer::Wgpu] {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([760.0, 560.0])
                .with_title("Kingdom 3DS installer"),
            renderer,
            ..Default::default()
        };
        match eframe::run_native(
            "Kingdom 3DS installer",
            options,
            Box::new(|_| Ok(Box::new(App::new()))),
        ) {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(format!("{renderer:?}: {e}")),
        }
    }
    Err(format!("couldn't open the window ({})", errors.join("; ")))
}
