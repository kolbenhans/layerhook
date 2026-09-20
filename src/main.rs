// Windows: no console window behind the GUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use layerhook::config::{AppConfig, DeviceRef, Rule};
use layerhook::tray::Tray;
use layerhook::{autostart, hid, tray, window};
use qmk_via_api::scan::{scan_keyboards, KeyboardDeviceInfo};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// BCORNE's keyColors keymap defines 10 dynamic layers (config.h
// DYNAMIC_KEYMAP_LAYER_COUNT). Hardcoded since our raw HID command doesn't
// query it — fine for this one board, revisit if layerhook ever targets more.
const LAYER_COUNT: u8 = 10;
const PATTERN_FIELD_WIDTH: f32 = 440.0;
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// Hyprland doesn't honor xdg_toplevel's minimize request for toplevels, so
// `ViewportCommand::Minimized` is a no-op there. The windowrule for the
// "layerhook" class (Hyprland-only config, see hypr-user.lua) parks it on a
// dedicated special workspace instead; toggling that workspace is what
// actually shows/hides the window there. On any other Linux desktop (KDE,
// GNOME, COSMOS, ...) there's no such rule, so fall back to plain Minimized
// — window-focus detection (window.rs) is Hyprland-only anyway, but hide/
// show degrading gracefully instead of silently doing nothing is cheap.
#[cfg(target_os = "linux")]
fn is_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

// Stock Hyprland takes `hyprctl dispatch togglespecialworkspace <name>`
// directly. Some builds (e.g. this machine's, via a Lua config layer) reject
// that plain form and only accept a Lua dispatcher call instead — fall back
// to that form if the plain one errors, so this keeps working on both.
#[cfg(target_os = "linux")]
fn hyprctl_toggle_special_workspace(name: &str) {
    let plain = std::process::Command::new("hyprctl").args(["dispatch", "togglespecialworkspace", name]).output();
    if matches!(&plain, Ok(o) if o.status.success()) {
        return;
    }
    let expr = format!(r#"hl.dsp.workspace.toggle_special("{name}")"#);
    let _ = std::process::Command::new("hyprctl").args(["dispatch", &expr]).spawn();
}

#[cfg(target_os = "linux")]
fn show_window(ctx: &egui::Context) {
    if is_hyprland() {
        hyprctl_toggle_special_workspace("layerhook");
    } else {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    ctx.request_repaint();
}

#[cfg(target_os = "linux")]
fn hide_window(ctx: &egui::Context) {
    if is_hyprland() {
        hyprctl_toggle_special_workspace("layerhook");
    } else {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

#[cfg(not(target_os = "linux"))]
fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

#[cfg(not(target_os = "linux"))]
fn hide_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
}

struct Shared {
    rules: Vec<Rule>,
    default_layer: u8,
    device: Option<KeyboardDeviceInfo>,
    last_title: Option<String>,
    last_layer_sent: Option<u8>,
    hid_error: Option<String>,
}

fn resolve_layer(rules: &[Rule], title: &str) -> Option<u8> {
    rules
        .iter()
        .find(|r| {
            regex::RegexBuilder::new(&r.pattern)
                .case_insensitive(true)
                .build()
                .is_ok_and(|re| re.is_match(title))
        })
        .map(|r| r.layer)
}

fn open_device(api: &hidapi::HidApi, dev: &KeyboardDeviceInfo) -> Option<hidapi::HidDevice> {
    let info = api.device_list().find(|d| {
        d.usage_page() == dev.usage_page && d.vendor_id() == dev.vendor_id && d.product_id() == dev.product_id
    })?;
    info.open_device(api).ok()
}

fn spawn_matcher(shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || {
        let Ok(api) = hidapi::HidApi::new() else { return };
        loop {
            std::thread::sleep(POLL_INTERVAL);

            let title = window::active_window_title();
            let (rules, default_layer, device, last_layer_sent) = {
                let s = shared.lock().unwrap();
                (s.rules.clone(), s.default_layer, s.device.clone(), s.last_layer_sent)
            };

            // No rule match (or no detectable window) falls back to the
            // configured default layer, so focusing something unrelated
            // always resets the keyboard instead of leaving it stuck on
            // whatever layer the last matched app wanted.
            let target_layer = title.as_deref().and_then(|t| resolve_layer(&rules, t)).unwrap_or(default_layer);

            let mut attempted = false;
            let mut hid_error = None;
            if let Some(dev) = device.as_ref() {
                if Some(target_layer) != last_layer_sent {
                    attempted = true;
                    match open_device(&api, dev) {
                        Some(handle) => {
                            if let Err(e) = hid::set_layer(&handle, target_layer) {
                                hid_error = Some(e.to_string());
                            }
                        }
                        None => hid_error = Some("Device unreachable".to_string()),
                    }
                }
            }

            let mut s = shared.lock().unwrap();
            s.last_title = title;
            if attempted {
                s.hid_error = hid_error.clone();
                if hid_error.is_none() {
                    s.last_layer_sent = Some(target_layer);
                }
            }
        }
    });
}

struct App {
    devices: Vec<KeyboardDeviceInfo>,
    selected_device: Option<usize>,
    rules: Vec<Rule>,
    default_layer: u8,
    autostart: bool,
    new_pattern: String,
    new_layer: u8,
    window_titles: Vec<String>,
    shared: Arc<Mutex<Shared>>,
    _tray: Tray,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let cfg = AppConfig::load();
        let devices = scan_keyboards().unwrap_or_default();

        let tray = {
            let ctx = ctx.clone();
            tray::create_tray_icon(Arc::new(move || show_window(&ctx)))
        };

        let selected_device = cfg.device.as_ref().and_then(|d| {
            devices.iter().position(|dev| {
                dev.vendor_id == d.vendor_id
                    && dev.product_id == d.product_id
                    && (d.serial_number.is_none() || dev.serial_number == d.serial_number)
            })
        });

        let shared = Arc::new(Mutex::new(Shared {
            rules: cfg.rules.clone(),
            default_layer: cfg.default_layer,
            device: selected_device.and_then(|i| devices.get(i)).cloned(),
            last_title: None,
            last_layer_sent: None,
            hid_error: None,
        }));
        spawn_matcher(shared.clone());

        App {
            devices,
            selected_device,
            rules: cfg.rules,
            default_layer: cfg.default_layer,
            autostart: autostart::is_enabled(),
            new_pattern: String::new(),
            new_layer: 0,
            window_titles: window::list_window_titles(),
            shared,
            _tray: tray,
        }
    }

    fn persist(&self) {
        let device = self.selected_device.and_then(|i| self.devices.get(i)).map(|d| DeviceRef {
            vendor_id: d.vendor_id,
            product_id: d.product_id,
            serial_number: d.serial_number.clone(),
        });
        AppConfig { rules: self.rules.clone(), device, default_layer: self.default_layer }.save();
    }

    fn sync_shared(&self) {
        let mut s = self.shared.lock().unwrap();
        s.rules = self.rules.clone();
        s.default_layer = self.default_layer;
        s.device = self.selected_device.and_then(|i| self.devices.get(i)).cloned();
    }
}

fn layer_combo(ui: &mut egui::Ui, id_source: impl std::hash::Hash + std::fmt::Debug, layer: &mut u8) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_source).selected_text(format!("Layer {layer}")).show_ui(ui, |ui| {
        for l in 0..LAYER_COUNT {
            if ui.selectable_value(layer, l, format!("Layer {l}")).clicked() {
                changed = true;
            }
        }
    });
    changed
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(POLL_INTERVAL);

        // Closing the window hides it instead of quitting — the matcher
        // thread keeps running in the background; the tray's "Quit" is the
        // actual exit.
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_window(ui.ctx());
        }

        let section_title = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).strong().size(14.0));
            ui.add_space(6.0);
        };

        egui::CentralPanel::default().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            ui.horizontal(|ui| {
                ui.heading("layerhook");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.autostart, "Start automatically (login)").changed() {
                        autostart::set_enabled(self.autostart);
                    }
                });
            });
            ui.add_space(4.0);

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "Keyboard");
                ui.horizontal(|ui| {
                    let selected_text = self
                        .selected_device
                        .and_then(|i| self.devices.get(i))
                        .map(|d| format!("{} ({:04X}:{:04X})", d.product.clone().unwrap_or_default(), d.vendor_id, d.product_id))
                        .unwrap_or_else(|| "(none selected)".to_string());
                    egui::ComboBox::from_id_salt("device").selected_text(selected_text).show_ui(ui, |ui| {
                        for i in 0..self.devices.len() {
                            let d = &self.devices[i];
                            let label = format!("{} ({:04X}:{:04X})", d.product.clone().unwrap_or_default(), d.vendor_id, d.product_id);
                            if ui.selectable_label(self.selected_device == Some(i), label).clicked() {
                                self.selected_device = Some(i);
                                self.sync_shared();
                                self.persist();
                            }
                        }
                    });
                    if ui.button("Rescan").clicked() {
                        self.devices = scan_keyboards().unwrap_or_default();
                        self.selected_device = None;
                        self.sync_shared();
                    }
                });
            });

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "Add rule");

                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_pattern).hint_text("Regex, e.g. .*vscod.*|.*gedit.*").desired_width(PATTERN_FIELD_WIDTH));
                    layer_combo(ui, "new_layer", &mut self.new_layer);
                    if ui.add_enabled(!self.new_pattern.is_empty(), egui::Button::new("+ Add")).clicked() {
                        self.rules.push(Rule { pattern: std::mem::take(&mut self.new_pattern), layer: self.new_layer });
                        self.sync_shared();
                        self.persist();
                    }
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Pick from an open window:");
                    let mut picked: Option<String> = None;
                    egui::ComboBox::from_id_salt("open_windows").selected_text("Select window...").show_ui(ui, |ui| {
                        for title in &self.window_titles {
                            if ui.selectable_label(false, title).clicked() {
                                picked = Some(title.clone());
                            }
                        }
                    });
                    if let Some(title) = picked {
                        self.new_pattern = regex::escape(&title);
                    }
                    if ui.button("Refresh").clicked() {
                        self.window_titles = window::list_window_titles();
                    }
                });
            });

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    section_title(ui, "Rules");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        layer_combo(ui, "default_layer", &mut self.default_layer);
                        ui.label("Default layer (no rule matches):");
                    });
                });
                ui.add_space(2.0);

                let mut removed: Option<usize> = None;
                for (i, rule) in self.rules.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut rule.pattern).desired_width(PATTERN_FIELD_WIDTH));
                        layer_combo(ui, ("layer", i), &mut rule.layer);
                        if ui.button("Remove").clicked() {
                            removed = Some(i);
                        }
                    });
                }
                if self.rules.is_empty() {
                    ui.weak("No rules yet — add one above.");
                }
                // Catches in-place edits (pattern text, layer dropdown) that
                // have no dedicated click handler of their own.
                self.sync_shared();
                self.persist();
                if let Some(i) = removed {
                    self.rules.remove(i);
                    self.sync_shared();
                    self.persist();
                }
            });

            ui.add_space(2.0);
            let s = self.shared.lock().unwrap();
            ui.label(format!("Active window: {}", s.last_title.as_deref().unwrap_or("-")));
            ui.label(format!("Last sent layer: {}", s.last_layer_sent.map(|l| l.to_string()).unwrap_or_else(|| "-".to_string())));
            if let Some(err) = &s.hid_error {
                ui.colored_label(egui::Color32::from_rgb(200, 60, 60), format!("HID error: {err}"));
            }
        });
    }
}

fn app_icon() -> egui::IconData {
    let icon = image::load_from_memory(include_bytes!("../resources/icon-256.png")).expect("failed to load app icon").into_rgba8();
    let (width, height) = icon.dimensions();
    egui::IconData { rgba: icon.into_raw(), width, height }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([840.0, 520.0]).with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("layerhook", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}
