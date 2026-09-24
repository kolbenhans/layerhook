// Windows: no console window behind the GUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use layerhook::config::{AppConfig, DeviceRef, Rule};
use layerhook::{autostart, hid, tray, window};
use qmk_via_api::scan::{scan_keyboards, KeyboardDeviceInfo};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// BCORNE's keyColors keymap defines 10 dynamic layers (config.h
// DYNAMIC_KEYMAP_LAYER_COUNT). Hardcoded since our raw HID command doesn't
// query it — fine for this one board, revisit if layerhook ever targets more.
const LAYER_COUNT: u8 = 10;
const PATTERN_FIELD_WIDTH: f32 = 440.0;
const LAYER_COMBO_WIDTH: f32 = 110.0;
const ROW_BUTTON_WIDTH: f32 = 80.0;
const DRAG_HANDLE_WIDTH: f32 = 22.0;
// egui's Frame::central_panel inner margin, per side.
const CENTRAL_PANEL_MARGIN: f32 = 8.0;
const MIN_WINDOW_WIDTH: f32 = 640.0;
const MAX_WINDOW_WIDTH: f32 = 4000.0;
const POLL_INTERVAL: Duration = Duration::from_millis(500);
// Separate from POLL_INTERVAL on purpose: this only governs how often the idle
// GUI redraws itself (each redraw forces an OpenGL buffer swap even when
// nothing changed). Under GPU contention (e.g. a game) that swap can block
// long enough to miss the compositor's Wayland ping, marking the window
// "not responding" - unrelated apps that render nothing while idle don't
// have this problem since they never touch the GPU. The matcher thread's own
// polling (layer-switching responsiveness) is unaffected, it runs on
// POLL_INTERVAL regardless of this.
const GUI_REPAINT_INTERVAL: Duration = Duration::from_secs(2);

struct Shared {
    rules: Vec<Rule>,
    default_layer: u8,
    device: Option<KeyboardDeviceInfo>,
    last_title: Option<String>,
    last_layer_sent: Option<u8>,
    hid_error: Option<String>,
}

fn pattern_matches(pattern: &str, title: &str) -> bool {
    // fancy_regex, not the plain `regex` crate: rules commonly use lookarounds
    // (e.g. `(?!...)` to exclude a title) which `regex` refuses to compile at
    // all - that failure used to be swallowed as "no match", so an excluding
    // rule silently fell back to the default layer instead of ever matching.
    fancy_regex::Regex::new(&format!("(?i){pattern}")).is_ok_and(|re| re.is_match(title).unwrap_or(false))
}

fn resolve_layer(rules: &[Rule], title: &str) -> Option<u8> {
    rules.iter().find(|r| pattern_matches(&r.pattern, title)).map(|r| r.layer)
}

fn open_device(api: &hidapi::HidApi, dev: &KeyboardDeviceInfo) -> Option<hidapi::HidDevice> {
    let info = api.device_list().find(|d| {
        d.usage_page() == dev.usage_page && d.vendor_id() == dev.vendor_id && d.product_id() == dev.product_id
    })?;
    info.open_device(api).ok()
}

fn spawn_matcher(shared: Arc<Mutex<Shared>>) {
    // window::watch() pushes a title update the instant focus changes
    // instead of us polling for it - instant layer switches, and this thread
    // sits at zero CPU between events instead of waking every POLL_INTERVAL
    // just to ask "did anything change?". recv_timeout still retries on
    // POLL_INTERVAL even without a fresh event, so a failed HID write (e.g.
    // keyboard briefly unplugged) keeps getting retried rather than only on
    // the next focus change.
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    window::watch(tx);

    std::thread::spawn(move || {
        let Ok(api) = hidapi::HidApi::new() else { return };
        let mut last_title: Option<String> = None;
        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(title) => last_title = title,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
            let title = last_title.clone();

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
    start_minimized: bool,
    always_on_top: bool,
    new_pattern: String,
    new_layer: u8,
    window_titles: Vec<String>,
    shared: Arc<Mutex<Shared>>,
    icon_texture: egui::TextureHandle,
    // Last window height we requested to fit the content, so a resize is
    // only sent when the content actually changes (not every frame, which
    // would fight a manual resize).
    fitted_height: f32,
}

impl App {
    /// `shared` is created once in `main` and outlives every individual
    /// window — the window itself gets destroyed on close and a fresh one
    /// created on the next tray "Show" (see main()'s loop), so this re-reads
    /// current state from `shared` and rescans devices each time, rather
    /// than assuming a fresh launch.
    fn new(shared: Arc<Mutex<Shared>>, ctx: &egui::Context) -> Self {
        let devices = scan_keyboards().unwrap_or_default();
        let (rules, default_layer, current_device) = {
            let s = shared.lock().unwrap();
            (s.rules.clone(), s.default_layer, s.device.clone())
        };

        let selected_device = current_device.as_ref().and_then(|d| {
            devices.iter().position(|dev| {
                dev.vendor_id == d.vendor_id && dev.product_id == d.product_id && dev.serial_number == d.serial_number
            })
        });

        App {
            devices,
            selected_device,
            rules,
            default_layer,
            autostart: autostart::is_enabled(),
            start_minimized: AppConfig::load().start_minimized,
            always_on_top: AppConfig::load().always_on_top,
            new_pattern: String::new(),
            new_layer: 0,
            window_titles: window::list_window_titles(),
            shared,
            icon_texture: load_icon_texture(ctx),
            fitted_height: 0.0,
        }
    }

    /// Resizes the window to `desired` height when the content's height
    /// changed (rules added/removed, hint text appearing, ...). Width is left
    /// as the user has it.
    fn fit_window_height(&mut self, ctx: &egui::Context, mut desired: f32) {
        if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
            desired = desired.min(monitor.y - 80.0);
        }
        if (desired - self.fitted_height).abs() > 1.0 {
            self.fitted_height = desired;
            let width = ctx.input(|i| i.viewport().inner_rect).map_or(840.0, |r| r.width());
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, desired)));
            // Hyprland (and Wayland compositors generally) ignore a client's
            // resize request for an existing window - but they do honor the
            // xdg_toplevel min/max size hints, so pinning both to the wanted
            // height is what actually makes the window that tall. Width stays
            // freely resizable.
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(MIN_WINDOW_WIDTH, desired)));
            ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::vec2(MAX_WINDOW_WIDTH, desired)));
        }
    }

    fn persist(&self) {
        let device = self.selected_device.and_then(|i| self.devices.get(i)).map(|d| DeviceRef {
            vendor_id: d.vendor_id,
            product_id: d.product_id,
            serial_number: d.serial_number.clone(),
        });
        AppConfig { rules: self.rules.clone(), device, default_layer: self.default_layer, start_minimized: self.start_minimized, always_on_top: self.always_on_top }.save();
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
    egui::ComboBox::from_id_salt(id_source)
        .selected_text(format!("Layer {layer}"))
        .width(LAYER_COMBO_WIDTH)
        .show_ui(ui, |ui| {
            for l in 0..LAYER_COUNT {
                if ui.selectable_value(layer, l, format!("Layer {l}")).clicked() {
                    changed = true;
                }
            }
        });
    changed
}

fn weak(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).weak().size(size)
}

fn strong(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).strong().size(size)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(GUI_REPAINT_INTERVAL);

        // Closing the window really closes it (destroys this viewport) —
        // main()'s loop then waits for the next tray "Show" and builds a
        // fresh one. The matcher thread isn't tied to this window at all,
        // so layer-switching keeps running the whole time regardless.

        let section_header = |ui: &mut egui::Ui, title: &str, description: Option<&str>| {
            ui.horizontal(|ui| {
                ui.label(strong(title, 15.0));

                if let Some(description) = description {
                    ui.add_space(4.0);
                    ui.label(weak(description, 12.0));
                }
            });

            ui.add_space(8.0);
        };

        let content = egui::CentralPanel::default().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            // ─────────────────────────────────────────────────────────────
            // Header
            // ─────────────────────────────────────────────────────────────

            ui.horizontal(|ui| {
                ui.add(egui::Image::from_texture(&self.icon_texture).max_size(egui::vec2(32.0, 32.0)));
                ui.vertical(|ui| {
                    ui.heading("layerhook");
                    ui.label(weak("Automatic QMK layer switching", 12.0));
                });

                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.add_enabled_ui(self.autostart, |ui| {
                            if ui
                                .checkbox(&mut self.start_minimized, "Start minimized")
                                .changed()
                            {
                                self.persist();
                            }
                        });

                        if ui
                            .checkbox(
                                &mut self.autostart,
                                "Start automatically",
                            )
                            .changed()
                        {
                            autostart::set_enabled(self.autostart);
                            self.persist();
                        }

                        let supported = always_on_top_supported();
                        // Where it can't work, show it unchecked rather than
                        // displaying a stored "on" that does nothing.
                        let mut unsupported_off = false;
                        let checked = if supported { &mut self.always_on_top } else { &mut unsupported_off };
                        let pin = ui
                            .add_enabled(supported, egui::Checkbox::new(checked, "Always on top"))
                            .on_disabled_hover_text("Not available on Wayland - use your compositor's own pin / keep-above function for this window");
                        if pin.changed() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(window_level(self.always_on_top)));
                            self.persist();
                        }
                    },
                );
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // ─────────────────────────────────────────────────────────────
            // Keyboard
            // ─────────────────────────────────────────────────────────────

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());

                section_header(
                    ui,
                    "Keyboard",
                    Some("Select the QMK keyboard layerhook should control"),
                );

                let selected_text = self
                    .selected_device
                    .and_then(|i| self.devices.get(i))
                    .map(|d| {
                        format!(
                            "{} ({:04X}:{:04X})",
                            d.product.clone().unwrap_or_default(),
                            d.vendor_id,
                            d.product_id
                        )
                    })
                    .unwrap_or_else(|| "No keyboard selected".to_string());

                ui.horizontal(|ui| {
                    let mut rescan_clicked = false;

                    // Right-to-left: the button is placed flush against the
                    // row's right edge first, then the combo fills whatever
                    // width is left — same right margin on every row in this
                    // frame, regardless of each widget's natural size.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        rescan_clicked = ui.add(egui::Button::new("Rescan").min_size(egui::vec2(ROW_BUTTON_WIDTH, 0.0))).clicked();

                        let combo_width = ui.available_width().max(180.0);
                        egui::ComboBox::from_id_salt("device")
                            .selected_text(selected_text)
                            .width(combo_width)
                            .show_ui(ui, |ui| {
                                if self.devices.is_empty() {
                                    ui.weak("No compatible keyboards found.");
                                }

                                for i in 0..self.devices.len() {
                                    let d = &self.devices[i];

                                    let label = format!(
                                        "{} ({:04X}:{:04X})",
                                        d.product.clone().unwrap_or_default(),
                                        d.vendor_id,
                                        d.product_id
                                    );

                                    if ui
                                        .selectable_label(
                                            self.selected_device == Some(i),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        self.selected_device = Some(i);
                                        self.sync_shared();
                                        self.persist();
                                    }
                                }
                            });
                    });

                    if rescan_clicked {
                        self.devices = scan_keyboards().unwrap_or_default();
                        self.selected_device = None;
                        self.sync_shared();
                    }
                });

                if self.devices.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Connect your keyboard and press Rescan.",
                        )
                        .weak()
                        .italics(),
                    );
                }
            });

            ui.add_space(4.0);

            // ─────────────────────────────────────────────────────────────
            // Add rule
            // ─────────────────────────────────────────────────────────────

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());

                section_header(
                    ui,
                    "Add rule",
                    Some("Match the active window title with a regular expression"),
                );

                ui.horizontal(|ui| {
                    let mut add_clicked = false;

                    // Right-to-left: Add button and layer combo (both fixed
                    // width) land flush against the row's right edge, the
                    // pattern field fills whatever's left — same right
                    // margin as every other row in this frame.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        add_clicked = ui
                            .add_enabled(!self.new_pattern.is_empty(), egui::Button::new("+ Add").min_size(egui::vec2(ROW_BUTTON_WIDTH, 0.0)))
                            .clicked();

                        layer_combo(ui, "new_layer", &mut self.new_layer);

                        let pattern_width = ui.available_width().max(120.0);
                        ui.add_sized(
                            [pattern_width, 30.0],
                            egui::TextEdit::singleline(&mut self.new_pattern)
                                .hint_text("Regex, e.g. .*vscod.*|.*gedit.*"),
                        );
                    });

                    if add_clicked {
                        self.rules.push(Rule {
                            pattern: std::mem::take(&mut self.new_pattern),
                            layer: self.new_layer,
                        });

                        self.sync_shared();
                        self.persist();
                    }
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(weak("Open window", 12.0));

                    let mut refresh_clicked = false;
                    let mut picked: Option<String> = None;

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        refresh_clicked = ui.add(egui::Button::new("Refresh").min_size(egui::vec2(ROW_BUTTON_WIDTH, 0.0))).clicked();

                        let combo_width = ui.available_width().max(160.0);
                        egui::ComboBox::from_id_salt("open_windows")
                            .selected_text("Select a window...")
                            .width(combo_width)
                            .show_ui(ui, |ui| {
                                if self.window_titles.is_empty() {
                                    ui.weak("No windows available.");
                                }

                                for title in &self.window_titles {
                                    if ui
                                        .selectable_label(false, title)
                                        .clicked()
                                    {
                                        picked = Some(title.clone());
                                    }
                                }
                            });
                    });

                    if let Some(title) = picked {
                        self.new_pattern = regex::escape(&title);
                    }

                    if refresh_clicked {
                        self.window_titles = window::list_window_titles();
                    }
                });

                ui.add_space(4.0);

                ui.label(weak("Tip: Use negative lookaheads to exclude applications or browsers.", 11.0));
            });

            ui.add_space(4.0);

            // ─────────────────────────────────────────────────────────────
            // Rules
            // ─────────────────────────────────────────────────────────────

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(strong("Rules", 15.0));
                        ui.label(weak("Rules are evaluated against the active window title", 12.0));
                    });

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            layer_combo(
                                ui,
                                "default_layer",
                                &mut self.default_layer,
                            );

                            ui.label(weak("Default layer", 12.0));
                        },
                    );
                });

                ui.add_space(10.0);

                let mut removed: Option<usize> = None;
                // (from, insert_before) in pre-move indices, set by a drop.
                let mut moved: Option<(usize, usize)> = None;
                let mut changed = false;

                if self.rules.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);

                        ui.label(
                            egui::RichText::new("No rules configured")
                                .strong(),
                        );

                        ui.label(
                            egui::RichText::new(
                                "Add a rule above to automatically switch layers.",
                            )
                            .weak(),
                        );

                        ui.add_space(8.0);
                    });
                } else {
                    // Column labels
                    ui.horizontal(|ui| {
                        // Same width as each row's drag handle, so the
                        // column labels stay lined up with their fields.
                        ui.add_sized([DRAG_HANDLE_WIDTH, 18.0], egui::Label::new(""));
                        ui.add_sized([PATTERN_FIELD_WIDTH, 18.0], egui::Label::new(weak("Pattern", 11.0)));
                        ui.add_sized([LAYER_COMBO_WIDTH, 18.0], egui::Label::new(weak("Layer", 11.0)));
                        ui.add_space(8.0);
                        ui.label(weak("Action", 11.0));
                    });

                    ui.add_space(2.0);

                    // Capped so a long rule list scrolls instead of pushing
                    // the Status section off the (fixed-height) window.
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        let rule_count = self.rules.len();
                        for (i, rule) in self.rules.iter_mut().enumerate() {
                            let regex_error = fancy_regex::Regex::new(&format!("(?i){}", rule.pattern)).err();

                            let row = egui::Frame::new()
                                .fill(if regex_error.is_some() {
                                    egui::Color32::from_rgb(64, 24, 24)
                                } else {
                                    ui.visuals().faint_bg_color
                                })
                                .inner_margin(egui::Margin::symmetric(8, 5))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        // Only the handle is the drag source, not
                                        // the whole row - a drag-sensing rect over
                                        // the row would shadow the text field,
                                        // combo and button inside it.
                                        ui.dnd_drag_source(egui::Id::new(("rule_drag", i)), i, |ui| {
                                            ui.add_sized([DRAG_HANDLE_WIDTH, 28.0], egui::Label::new(weak("☰", 16.0)).selectable(false));
                                        });

                                        let pattern_field = ui.add_sized(
                                            [PATTERN_FIELD_WIDTH, 28.0],
                                            egui::TextEdit::singleline(&mut rule.pattern),
                                        );
                                        changed |= pattern_field.changed();
                                        if let Some(err) = &regex_error {
                                            let _ = pattern_field.on_hover_text(format!("Invalid regex: {err}"));
                                        }

                                        if layer_combo(
                                            ui,
                                            ("layer", i),
                                            &mut rule.layer,
                                        ) {
                                            changed = true;
                                        }

                                        if ui.button("Remove").clicked() {
                                            removed = Some(i);
                                        }
                                    });
                                })
                                .response;

                            // Drop target: the row half the pointer is in decides
                            // whether the dragged rule lands before or after this
                            // one; a line shows where.
                            if let (Some(pointer), Some(dragged)) = (ui.input(|i| i.pointer.interact_pos()), row.dnd_hover_payload::<usize>()) {
                                let stroke = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
                                let insert_before = if *dragged == i || pointer.y < row.rect.center().y {
                                    ui.painter().hline(row.rect.x_range(), row.rect.top(), stroke);
                                    i
                                } else {
                                    ui.painter().hline(row.rect.x_range(), row.rect.bottom(), stroke);
                                    i + 1
                                };
                                if let Some(from) = row.dnd_release_payload::<usize>() {
                                    moved = Some((*from, insert_before));
                                }
                            }

                            ui.add_space(3.0);
                        }

                        // A list taller than the scroll area can't otherwise be
                        // reordered across its hidden part: scroll while a drag
                        // is held near the top/bottom edge.
                        if rule_count > 0 && egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx()) {
                            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                                let visible = ui.clip_rect();
                                let edge = 24.0;
                                if pointer.y < visible.top() + edge {
                                    ui.scroll_with_delta(egui::vec2(0.0, 10.0));
                                    ui.ctx().request_repaint();
                                } else if pointer.y > visible.bottom() - edge {
                                    ui.scroll_with_delta(egui::vec2(0.0, -10.0));
                                    ui.ctx().request_repaint();
                                }
                            }
                        }
                    });
                }

                if let Some(i) = removed {
                    self.rules.remove(i);
                    changed = true;
                } else if let Some((from, insert_before)) = moved {
                    // `insert_before` indexes the list as it was before the
                    // removal, so it shifts down by one if the rule came from
                    // above it.
                    let rule = self.rules.remove(from);
                    let to = if from < insert_before { insert_before - 1 } else { insert_before };
                    self.rules.insert(to, rule);
                    changed = true;
                }

                if changed {
                    self.sync_shared();
                    self.persist();
                }
            });

            ui.add_space(4.0);

            // ─────────────────────────────────────────────────────────────
            // Status
            // ─────────────────────────────────────────────────────────────

            let s = self.shared.lock().unwrap();
            let content_top = ui.max_rect().top();

            let status_frame = egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.label(strong("Status", 13.0));

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if s.hid_error.is_some() {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 60, 60),
                                    "● HID error",
                                );
                            } else {
                                ui.colored_label(
                                    egui::Color32::from_rgb(80, 170, 100),
                                    "● Connected",
                                );
                            }
                        },
                    );
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(weak("Active window:", 12.0));
                    ui.label(s.last_title.as_deref().unwrap_or("-"));
                    // Windows only: the focused window itself had no title
                    // (e.g. a Photoshop tool panel) and this is its owner
                    // window's title instead - see window::owner_note().
                    if let Some(owner) = window::owner_note() {
                        ui.label(weak(&format!("(owner: {owner})"), 12.0));
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(weak("Last sent layer:", 12.0));
                    ui.label(s.last_layer_sent.map(|l| l.to_string()).unwrap_or_else(|| "-".to_string()));
                });

                if let Some(err) = &s.hid_error {
                    ui.add_space(4.0);

                    ui.colored_label(
                        egui::Color32::from_rgb(200, 60, 60),
                        format!("HID error: {err}"),
                    );
                }
            });

            // The Status frame is the last thing in the panel, so its bottom
            // edge is the content's height. (ui.min_rect() isn't usable: the
            // header's right-to-left layout claims the panel's full height.)
            status_frame.response.rect.bottom() - content_top
        });

        self.fit_window_height(ui.ctx(), content.inner + 2.0 * CENTRAL_PANEL_MARGIN);
    }
}

/// Whether the window can be pinned above others. winit implements it on
/// Windows and X11 (`_NET_WM_STATE_ABOVE`) but not on Wayland: there's no
/// protocol for a normal app window to ask for it, only the compositor's own
/// keep-above/pin function can do that.
fn always_on_top_supported() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_none()
}

fn window_level(always_on_top: bool) -> egui::WindowLevel {
    if always_on_top {
        egui::WindowLevel::AlwaysOnTop
    } else {
        egui::WindowLevel::Normal
    }
}

fn app_icon() -> egui::IconData {
    let icon = image::load_from_memory(include_bytes!("../resources/icon-256.png")).expect("failed to load app icon").into_rgba8();
    let (width, height) = icon.dimensions();
    egui::IconData { rgba: icon.into_raw(), width, height }
}

// Same PNG as the window/tray icon, just as a texture for the in-GUI header
// instead of an egui::IconData for the OS-level window icon.
fn load_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let icon = image::load_from_memory(include_bytes!("../resources/icon-256.png")).expect("failed to load app icon").into_rgba8();
    let (width, height) = icon.dimensions();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &icon.into_raw());
    ctx.load_texture("app-icon", color_image, egui::TextureOptions::LINEAR)
}

fn main() {
    let cfg = AppConfig::load();
    let devices = scan_keyboards().unwrap_or_default();
    let selected_device = cfg.device.as_ref().and_then(|d| {
        devices.iter().position(|dev| {
            dev.vendor_id == d.vendor_id && dev.product_id == d.product_id && (d.serial_number.is_none() || dev.serial_number == d.serial_number)
        })
    });
    let shared = Arc::new(Mutex::new(Shared {
        rules: cfg.rules,
        default_layer: cfg.default_layer,
        device: selected_device.and_then(|i| devices.get(i)).cloned(),
        last_title: None,
        last_layer_sent: None,
        hid_error: None,
    }));
    spawn_matcher(shared.clone());

    // Tray-driven show/hide used to fight the Wayland compositor (resizing
    // or unmapping an existing surface, both of which this Hyprland build
    // handled badly — see git history). Actually closing and recreating the
    // window sidesteps all of that: on close the viewport is genuinely
    // destroyed, and this loop just waits for the next tray "Show" to build
    // a new one. The tray and the matcher thread above are not tied to any
    // particular window, so they keep running across every close/reopen.
    //
    // "Start minimized" only skips this very first show — it only makes
    // sense paired with autostart, since a manual launch should always show
    // the window, so it's ignored if autostart isn't actually enabled.
    let show_on_launch = !(cfg.start_minimized && autostart::is_enabled());
    let show_requested = Arc::new((Mutex::new(show_on_launch), Condvar::new()));
    let _tray = {
        let show_requested = show_requested.clone();
        tray::create_tray_icon(Arc::new(move || {
            let (lock, cvar) = &*show_requested;
            *lock.lock().unwrap() = true;
            cvar.notify_one();
        }))
    };

    let icon = Arc::new(app_icon());
    loop {
        {
            let (lock, cvar) = &*show_requested;
            let mut requested = lock.lock().unwrap();
            while !*requested {
                requested = cvar.wait(requested).unwrap();
            }
            *requested = false;
        }

        // Re-read from disk: the GUI persists changes, so the copy loaded at
        // startup goes stale once the setting has been toggled.
        let always_on_top = always_on_top_supported() && AppConfig::load().always_on_top;
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([840.0, 720.0]).with_icon(icon.clone()).with_window_level(window_level(always_on_top)),
            ..Default::default()
        };
        let shared = shared.clone();
        let _ = eframe::run_native("layerhook", options, Box::new(move |cc| Ok(Box::new(App::new(shared, &cc.egui_ctx)))));
    }
}
