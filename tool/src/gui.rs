//! The egui front end: status panel, firmware choice, action buttons, attention banner, log.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};

use crate::descriptor::DescriptorState;
use crate::engine::{Engine, Event, Report, Sink};
use crate::image::{self, Firmware};
use crate::platform;

const GREEN: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
const RED: Color32 = Color32::from_rgb(0xd0, 0x3a, 0x3a);
const AMBER: Color32 = Color32::from_rgb(0xe0, 0x9a, 0x1a);
const GREY: Color32 = Color32::from_rgb(0x90, 0x90, 0x90);

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 700.0])
            .with_min_inner_size([600.0, 520.0])
            .with_title("M-Track 2X2 Firmware Fix"),
        ..Default::default()
    };
    eframe::run_native(
        "M-Track 2X2 Firmware Fix",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FwChoice {
    Builtin,
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Flash,
    Revert,
    RemoveVendor,
}

struct App {
    tx: Sender<Event>,
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    busy: bool,
    step: String,
    attention: Option<String>,
    progress: Option<f32>,
    log: Vec<String>,
    report: Option<Report>,
    result: Option<(bool, String)>,
    fw_choice: FwChoice,
    builtin: Firmware,
    custom_path: String,
    custom: Option<Result<Firmware, String>>,
    force: bool,
    confirm: Option<Pending>,
    started: bool,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            step: String::new(),
            attention: None,
            progress: None,
            log: Vec::new(),
            report: None,
            result: None,
            fw_choice: FwChoice::Builtin,
            builtin: image::builtin(),
            custom_path: String::new(),
            custom: None,
            force: false,
            confirm: None,
            started: false,
        }
    }

    fn sink(&self, ctx: &egui::Context) -> Sink {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        Arc::new(move |e: Event| {
            let _ = tx.send(e);
            ctx.request_repaint();
        })
    }

    /// Run a flow on a worker thread.
    fn spawn(&mut self, ctx: &egui::Context, f: impl FnOnce(&Engine) -> anyhow::Result<String> + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.result = None;
        self.progress = None;
        self.cancel.store(false, Ordering::Relaxed);
        let sink = self.sink(ctx);
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let engine = Engine::new(sink.clone(), cancel);
            let r = f(&engine);
            let _ = engine.check();
            sink(match r {
                Ok(s) => Event::Done { ok: true, summary: s },
                Err(e) => Event::Done {
                    ok: false,
                    summary: format!("{e:#}"),
                },
            });
        });
    }

    fn poll(&mut self) {
        while let Ok(e) = self.rx.try_recv() {
            match e {
                Event::Log(s) => self.log.push(s),
                Event::Step(s) => self.step = s,
                Event::Attention(a) => self.attention = a,
                Event::Progress(p) => self.progress = p,
                Event::Report(r) => self.report = Some(*r),
                Event::Done { ok, summary } => {
                    self.busy = false;
                    self.step.clear();
                    self.progress = None;
                    self.log
                        .push(format!("{} {summary}", if ok { "DONE:" } else { "FAILED:" }));
                    self.result = Some((ok, summary));
                }
            }
        }
    }

    fn load_custom(&mut self) {
        let p = self.custom_path.trim().to_string();
        if p.is_empty() {
            self.custom = None;
            return;
        }
        self.custom = Some(match std::fs::read(&p) {
            Ok(b) => image::load(
                &b,
                PathBuf::from(&p)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .as_deref()
                    .unwrap_or(&p),
            )
            .map_err(|e| format!("{e:#}")),
            Err(e) => Err(format!("cannot read {p}: {e}")),
        });
    }

    fn selected_firmware(&self) -> Option<Firmware> {
        match self.fw_choice {
            FwChoice::Builtin => Some(self.builtin.clone()),
            FwChoice::Custom => match &self.custom {
                Some(Ok(fw)) => Some(fw.clone()),
                _ => None,
            },
        }
    }

    // ------------------------------------------------------------ UI pieces

    fn status_panel(&self, ui: &mut egui::Ui) {
        ui.heading("Status");
        let Some(r) = &self.report else {
            ui.label("Checking ...");
            return;
        };
        let line = |ui: &mut egui::Ui, ok: Option<bool>, text: &str| {
            // Plain ASCII marks: egui's default fonts have no check-mark glyphs.
            let (mark, color) = match ok {
                Some(true) => ("OK", GREEN),
                Some(false) => ("!!", RED),
                None => ("--", GREY),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(mark).color(color).strong().monospace());
                ui.label(text);
            });
        };
        match (&r.runtime, &r.dfu_mode) {
            (Some(d), _) => line(ui, Some(true), &format!("M-Track 2X2 connected: {d}")),
            (None, Some(d)) => line(ui, None, &format!("M-Track 2X2 connected in DFU mode: {d}")),
            (None, None) => line(
                ui,
                Some(false),
                "No M-Track 2X2 found. Plug it in and press Check again.",
            ),
        }
        match (r.descriptor, &r.descriptor_error) {
            (Some(DescriptorState::Fixed), _) => line(
                ui,
                Some(true),
                "Firmware: FIXED. The capture terminal is wired to the microphone.",
            ),
            (Some(DescriptorState::Buggy), _) => line(
                ui,
                Some(false),
                "Firmware: BUG PRESENT. The capture terminal points at USB playback, so Windows creates no microphone.",
            ),
            (Some(s), _) => line(ui, None, &format!("Firmware: {}", s.describe())),
            (None, Some(e)) => line(ui, None, &format!("Firmware: could not read the descriptor ({e})")),
            (None, None) => {}
        }
        for i in &r.host.items {
            line(ui, i.ok, &i.text);
        }
    }

    fn firmware_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Firmware image");
        ui.radio_value(
            &mut self.fw_choice,
            FwChoice::Builtin,
            format!(
                "Built-in fixed firmware (upgrade image v{}, {} bytes, sha256 {}…)",
                self.builtin.info.version,
                self.builtin.info.length,
                &self.builtin.sha256_logical[..12]
            ),
        );
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.fw_choice, FwChoice::Custom, "Custom image file:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.custom_path)
                    .desired_width(360.0)
                    .hint_text("path to .bin (or drop a file here)"),
            );
            if resp.changed() {
                self.fw_choice = FwChoice::Custom;
                self.load_custom();
            }
        });
        let dropped: Vec<PathBuf> = ui
            .ctx()
            .input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.custom_path = p.to_string_lossy().into_owned();
            self.fw_choice = FwChoice::Custom;
            self.load_custom();
        }
        if self.fw_choice == FwChoice::Custom {
            match &self.custom {
                Some(Ok(fw)) => {
                    ui.colored_label(
                        GREEN,
                        format!(
                            "Valid xflash image: version {}, {} bytes, CRCs ok, sha256 {}…",
                            fw.info.version,
                            fw.info.length,
                            &fw.sha256_logical[..12]
                        ),
                    );
                }
                Some(Err(e)) => {
                    ui.colored_label(RED, e);
                }
                None => {
                    ui.colored_label(GREY, "Enter a path or drop an image file onto the window.");
                }
            }
        }
        ui.checkbox(&mut self.force, "Re-flash even if the device already holds this image");
    }

    fn action_buttons(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let report = self.report.clone();
        let vendor = report.as_ref().map(|r| r.host.vendor_driver).unwrap_or(false);
        let present = report.as_ref().map(|r| r.device_present()).unwrap_or(false);
        let fw_ok = self.selected_firmware().is_some();
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.busy, egui::Button::new("Check again")).clicked() {
                self.spawn(&ctx, |e| {
                    e.check();
                    Ok("Check complete.".into())
                });
            }
            if cfg!(windows) {
                if ui
                    .add_enabled(!self.busy && vendor, egui::Button::new("Remove M-Audio driver"))
                    .on_hover_text("Uninstalls the M-Audio driver package so Windows' own USB Audio 2.0 driver takes over. A reboot follows.")
                    .clicked()
                {
                    self.confirm = Some(Pending::RemoveVendor);
                }
                if ui
                    .add_enabled(!self.busy && !vendor && present, egui::Button::new("Set up DFU access"))
                    .on_hover_text("Binds Windows' in-box WinUSB driver to the DFU interface (no download). Also done automatically when flashing.")
                    .clicked()
                {
                    self.spawn(&ctx, |e| e.fix_host());
                }
            } else if ui
                .add_enabled(!self.busy, egui::Button::new("Set up USB access"))
                .on_hover_text("Installs a udev rule so the tool can talk to the device without root. Asks for the administrator password once (pkexec).")
                .clicked()
            {
                self.spawn(&ctx, |e| e.fix_host());
            }
            let flash_btn = egui::Button::new(RichText::new("Flash firmware").strong());
            if ui.add_enabled(!self.busy && !vendor && present && fw_ok, flash_btn).clicked() {
                self.confirm = Some(Pending::Flash);
            }
            if ui
                .add_enabled(!self.busy && !vendor && present, egui::Button::new("Revert to factory"))
                .on_hover_text("Deletes the upgrade image; the device boots its original firmware again.")
                .clicked()
            {
                self.confirm = Some(Pending::Revert);
            }
            if self.busy {
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            }
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.confirm else { return };
        let (title, body) = match pending {
            Pending::Flash => (
                "Flash the firmware?",
                "The tool switches the interface into DFU mode, writes the image as an upgrade image and reboots it. \
                 The factory firmware stays on the chip and is the automatic fallback if the image is rejected. \
                 Do not unplug the device unless the tool asks you to.",
            ),
            Pending::Revert => ("Revert to factory firmware?", "This deletes the upgrade image; the capture bug will be back."),
            Pending::RemoveVendor => (
                "Remove the M-Audio driver package?",
                "The M-Audio driver and its control panel are uninstalled; Windows' in-box USB Audio 2.0 driver takes over. \
                 You will need to reboot afterwards.",
            ),
        };
        let mut choice: Option<bool> = None;
        egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title);
            ui.label(body);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Yes, continue").strong()).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if let Some(yes) = choice {
            self.confirm = None;
            if yes {
                match pending {
                    Pending::Flash => {
                        if let Some(fw) = self.selected_firmware() {
                            let force = self.force;
                            self.spawn(ctx, move |e| e.flash(&fw, force));
                        }
                    }
                    Pending::Revert => self.spawn(ctx, |e| e.revert()),
                    Pending::RemoveVendor => self.spawn(ctx, |e| e.fix_host()),
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = root.ctx().clone();
        if !self.started {
            self.started = true;
            self.log.push(format!(
                "M-Track 2X2 firmware fix tool {} on {}",
                env!("CARGO_PKG_VERSION"),
                platform::OS_NAME
            ));
            self.spawn(&ctx, |e| {
                e.check();
                Ok("Check complete.".into())
            });
        }
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        egui::CentralPanel::default().show(root, |ui| {
            ui.heading(RichText::new("M-Audio M-Track 2X2 Firmware Fix").size(22.0));
            ui.label("Fixes the USB descriptor bug that stops Windows' built-in driver from creating a microphone input. Flashes a signed upgrade image over DFU; the factory firmware remains as fallback.");
            ui.separator();

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.status_panel(ui);
            });
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.firmware_panel(ui);
            });
            ui.add_space(4.0);
            self.action_buttons(ui);

            if let Some(a) = self.attention.clone() {
                egui::Frame::new().fill(AMBER.gamma_multiply(0.25)).inner_margin(10.0).corner_radius(6.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(a).color(AMBER).strong().size(16.0));
                });
            }
            if !self.step.is_empty() {
                ui.label(RichText::new(&self.step).strong());
            }
            if let Some(p) = self.progress {
                ui.add(egui::ProgressBar::new(p).show_percentage());
            }
            if let Some((ok, s)) = &self.result {
                ui.colored_label(if *ok { GREEN } else { RED }, s);
            }
            ui.separator();
            ui.label("Log");
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                let text = self.log.join("\n");
                ui.add(egui::TextEdit::multiline(&mut text.as_str()).code_editor().desired_width(f32::INFINITY));
            });
        });
        self.confirm_dialog(&ctx);
    }
}
