#![windows_subsystem = "windows"]

mod config;
mod ffmpeg;
mod indicator;
mod rec;
mod winapi;

use config::{Mode, Settings, SourceId};
use cpal::traits::{DeviceTrait, HostTrait};
use eframe::egui::{self, Color32, RichText};
use ffmpeg::FfState;
use rec::RecState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const APP_TITLE: &str = "Bildschirm-Recorder";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/egs2003/recorder";
const SMALL: [f32; 2] = [600.0, 800.0];
const BIG: [f32; 2] = [1150.0, 780.0];

// Farben
const BG: Color32 = Color32::from_rgb(20, 22, 27);
const CARD: Color32 = Color32::from_rgb(30, 33, 40);
const BORDER: Color32 = Color32::from_rgb(46, 50, 60);
const WIDGET: Color32 = Color32::from_rgb(41, 45, 55);
const WIDGET_HOVER: Color32 = Color32::from_rgb(52, 57, 70);
const ACCENT: Color32 = Color32::from_rgb(86, 132, 255);
const RED: Color32 = Color32::from_rgb(232, 64, 64);
const GREEN: Color32 = Color32::from_rgb(70, 190, 120);
const AMBER: Color32 = Color32::from_rgb(235, 170, 50);
const MUTED: Color32 = Color32::from_rgb(140, 147, 163);
const INFO_BG: Color32 = Color32::from_rgb(26, 35, 64);
const INFO_TEXT: Color32 = Color32::from_rgb(169, 184, 230);
const ERR_BG: Color32 = Color32::from_rgb(52, 26, 30);

type Shot = Result<(usize, usize, Vec<u8>), String>;

struct Picker {
    tex: egui::TextureHandle,
    a: Option<egui::Pos2>,
    b: Option<egui::Pos2>,
}

struct App {
    s: Settings,
    monitors: Vec<winapi::Monitor>,
    windows: Vec<String>,
    devices: Vec<(SourceId, String)>,
    rec: Option<rec::Handle>,
    ff: Arc<Mutex<FfState>>,
    picker: Option<Picker>,
    shot: Arc<Mutex<Option<Shot>>>,
    shot_pending: bool,
    msg: String,
}

fn list_audio() -> Vec<(SourceId, String)> {
    let host = cpal::default_host();
    let mut v = Vec::new();
    if let Ok(it) = host.output_devices() {
        for d in it {
            if let Ok(n) = d.name() {
                v.push((SourceId { output: true, name: n.clone() }, format!("Systemton  -  {n}")));
            }
        }
    }
    if let Ok(it) = host.input_devices() {
        for d in it {
            if let Ok(n) = d.name() {
                v.push((SourceId { output: false, name: n.clone() }, format!("Eingang  -  {n}")));
            }
        }
    }
    v
}

fn setup_style(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Button, FontId::new(15.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small, FontId::new(12.5, FontFamily::Proportional));
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 26.0;

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(24, 26, 32);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    let r = Rounding::same(9.0);
    v.widgets.noninteractive.rounding = r;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.rounding = r;
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.rounding = r;
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER;
    v.widgets.hovered.bg_fill = WIDGET_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.rounding = r;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.open.rounding = r;
    v.widgets.open.weak_bg_fill = WIDGET_HOVER;
    v.widgets.open.bg_fill = WIDGET_HOVER;
    style.visuals = v;
    ctx.set_style(style);
}

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title.to_uppercase()).size(12.0).color(MUTED).strong());
            ui.add_space(2.0);
            add(ui);
        });
}

fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, val: &mut T, items: &[(T, &str)]) {
    let n = items.len() as f32;
    let gap = ui.spacing().item_spacing.x;
    let w = (ui.available_width() - gap * (n - 1.0)) / n;
    ui.horizontal(|ui| {
        for (v, label) in items {
            let sel = *val == *v;
            let btn = egui::Button::new(RichText::new(*label).size(15.0).strong())
                .min_size(egui::vec2(w, 36.0))
                .selected(sel);
            if ui.add(btn).clicked() {
                *val = *v;
            }
        }
    });
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 5.5, color);
}

fn mb(b: u64) -> String {
    format!("{:.0} MB", b as f64 / 1_048_576.0)
}

fn ffmpeg_card(ui: &mut egui::Ui, st: &FfState, retry: &mut bool) {
    let failed = matches!(st, FfState::Failed(_));
    let (fill, accent) = if failed { (ERR_BG, RED) } else { (INFO_BG, ACCENT) };
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, accent))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                {
                    let p = ui.painter();
                    let c = r.center();
                    p.circle_filled(c, 17.0, accent);
                    if failed {
                        p.text(c, egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(20.0), Color32::WHITE);
                    } else {
                        let s = egui::Stroke::new(2.2, Color32::WHITE);
                        p.line_segment([c + egui::vec2(0.0, -8.0), c + egui::vec2(0.0, 6.0)], s);
                        p.line_segment([c + egui::vec2(-5.0, 1.0), c + egui::vec2(0.0, 7.0)], s);
                        p.line_segment([c + egui::vec2(5.0, 1.0), c + egui::vec2(0.0, 7.0)], s);
                    }
                }
                ui.vertical(|ui| {
                    let title = if failed { "ffmpeg konnte nicht eingerichtet werden" } else { "ffmpeg wird eingerichtet" };
                    ui.label(RichText::new(title).size(16.0).strong());
                    if !failed {
                        ui.label(RichText::new("Einmalig beim ersten Start. Danach ist die App bereit.").size(12.5).color(INFO_TEXT));
                    }
                });
            });
            match st {
                FfState::Downloading { done, total } => {
                    if *total > 0 {
                        ui.add(egui::ProgressBar::new(*done as f32 / *total as f32).desired_height(10.0).fill(ACCENT));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} von {}", mb(*done), mb(*total))).size(13.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("{} %", (*done * 100 / *total).min(100))).size(13.0));
                            });
                        });
                    } else {
                        ui.add(egui::ProgressBar::new(0.0).desired_height(10.0).fill(ACCENT).animate(true));
                        ui.label(RichText::new(format!("{} heruntergeladen", mb(*done))).size(13.0));
                    }
                }
                FfState::Extracting => {
                    ui.add(egui::ProgressBar::new(1.0).desired_height(10.0).fill(ACCENT).animate(true));
                    ui.label(RichText::new("Wird entpackt ...").size(13.0));
                }
                FfState::Failed(e) => {
                    ui.label(RichText::new(e).size(13.0).color(Color32::from_rgb(240, 170, 170)));
                    if ui.button("Erneut versuchen").clicked() {
                        *retry = true;
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Aufnahme und Einstellungen sind bis zum Abschluss gesperrt.").size(12.5).color(if failed { Color32::from_rgb(240, 170, 170) } else { INFO_TEXT }));
            });
        });
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let mut s = Settings::load();
        if !s.audio_initialized {
            if let Some(n) = cpal::default_host().default_output_device().and_then(|d| d.name().ok()) {
                s.audio.push(SourceId { output: true, name: n });
            }
            s.audio_initialized = true;
        }
        let ff = Arc::new(Mutex::new(FfState::Checking));
        ffmpeg::ensure(ff.clone(), ctx.clone());
        App {
            s,
            monitors: winapi::list_monitors(),
            windows: winapi::list_windows(),
            devices: list_audio(),
            rec: None,
            ff,
            picker: None,
            shot: Arc::new(Mutex::new(None)),
            shot_pending: false,
            msg: String::new(),
        }
    }

    fn start_grab(&mut self, ctx: &egui::Context) {
        self.shot_pending = true;
        self.monitors = winapi::list_monitors();
        let slot = self.shot.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            std::thread::sleep(Duration::from_millis(800));
            let r = rec::screenshot().map_err(|e| format!("{e:#}"));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            *slot.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
    }

    fn picker_ui(&mut self, ctx: &egui::Context) {
        let (ox, oy, vw, vh) = winapi::virtual_bounds(&self.monitors);
        let mut confirm = false;
        let mut cancel = false;
        let mut sel = (None, None);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::same(16.0)))
            .show(ctx, |ui| {
                let p = self.picker.as_mut().unwrap();
                ui.label(RichText::new("Bereich auswählen").size(20.0).strong());
                ui.label(RichText::new("Mit gedrückter Maustaste einen Bereich aufziehen.").color(MUTED));
                let tsize = p.tex.size_vec2();
                let avail = ui.available_size() - egui::vec2(0.0, 60.0);
                let scale = (avail.x / tsize.x).min(avail.y / tsize.y).max(0.05);
                let (resp, painter) = ui.allocate_painter(tsize * scale, egui::Sense::drag());
                let rect = resp.rect;
                painter.image(
                    p.tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                let to_norm = |pos: egui::Pos2| {
                    egui::pos2(
                        ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
                        ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
                    )
                };
                if resp.drag_started() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        p.a = Some(to_norm(pos));
                        p.b = p.a;
                    }
                } else if resp.dragged() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        p.b = Some(to_norm(pos));
                    }
                }
                if let (Some(a), Some(b)) = (p.a, p.b) {
                    let denorm = |n: egui::Pos2| {
                        egui::pos2(rect.min.x + n.x * rect.width(), rect.min.y + n.y * rect.height())
                    };
                    let sr = egui::Rect::from_two_pos(denorm(a), denorm(b));
                    painter.rect_filled(sr, 0.0, Color32::from_rgba_unmultiplied(86, 132, 255, 50));
                    painter.rect_stroke(sr, 0.0, egui::Stroke::new(2.0, ACCENT));
                }
                ui.horizontal(|ui| {
                    let ok = matches!((p.a, p.b), (Some(a), Some(b)) if (a.x - b.x).abs() > 0.005 && (a.y - b.y).abs() > 0.005);
                    let btn = egui::Button::new(RichText::new("Übernehmen").strong().color(Color32::WHITE)).fill(ACCENT);
                    if ui.add_enabled(ok, btn).clicked() {
                        confirm = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        cancel = true;
                    }
                });
                sel = (p.a, p.b);
            });
        if confirm {
            if let (Some(a), Some(b)) = sel {
                let r = egui::Rect::from_two_pos(a, b);
                self.s.region.x = ox + (r.min.x * vw as f32).round() as i32;
                self.s.region.y = oy + (r.min.y * vh as f32).round() as i32;
                self.s.region.w = ((r.width() * vw as f32).round() as i32) & !1;
                self.s.region.h = ((r.height() * vh as f32).round() as i32) & !1;
                self.s.mode = Mode::Region;
            }
        }
        if confirm || cancel {
            self.picker = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(SMALL[0], SMALL[1])));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Screenshot für die Bereichsauswahl abholen
        if self.shot_pending {
            let res = self.shot.lock().unwrap().take();
            match res {
                Some(Ok((w, h, px))) => {
                    self.shot_pending = false;
                    let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &px);
                    let tex = ctx.load_texture("shot", img, egui::TextureOptions::LINEAR);
                    self.picker = Some(Picker { tex, a: None, b: None });
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(BIG[0], BIG[1])));
                }
                Some(Err(e)) => {
                    self.shot_pending = false;
                    self.msg = e;
                }
                None => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        if self.picker.is_some() {
            self.picker_ui(ctx);
            return;
        }

        let st = self.rec.as_ref().map(|h| h.state.lock().unwrap().clone());
        let active = matches!(st, Some(RecState::Starting) | Some(RecState::Recording(_)) | Some(RecState::Saving));
        if active {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let ffst = self.ff.lock().unwrap().clone();
        let ff_ready = matches!(ffst, FfState::Ready);

        let mut start = false;
        let mut stop = false;
        let mut grab = false;
        let mut browse = false;
        let mut refresh_win = false;
        let mut refresh_audio = false;
        let mut retry_ff = false;
        let mut open_path: Option<(PathBuf, bool)> = None;

        egui::TopBottomPanel::bottom("bottom")
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(20.0, 12.0)))
            .show(ctx, |ui| {
                    let w = ui.available_width();
                    let big = |text: String, fill: Color32| {
                        egui::Button::new(RichText::new(text).size(18.0).strong().color(Color32::WHITE))
                            .fill(fill)
                            .rounding(12.0)
                            .min_size(egui::vec2(w, 52.0))
                    };
                    match &st {
                        Some(RecState::Starting) => {
                            ui.add_enabled(false, big("Starte ...".into(), AMBER));
                        }
                        Some(RecState::Saving) => {
                            ui.add_enabled(false, big("Speichere ...".into(), AMBER));
                        }
                        Some(RecState::Recording(t)) => {
                            let secs = t.elapsed().as_secs();
                            let label = format!("Aufnahme beenden    {:02}:{:02}", secs / 60, secs % 60);
                            if ui.add(big(label, RED)).clicked() {
                                stop = true;
                            }
                        }
                        _ => {
                            if ui
                                .add_enabled(ff_ready, big("Aufnahme starten".into(), if ff_ready { RED } else { WIDGET }))
                                .clicked()
                            {
                                start = true;
                            }
                        }
                    }

                    // Statuszeile
                    ui.horizontal(|ui| {
                        match &st {
                            Some(RecState::Recording(_)) => {
                                let on = (ctx.input(|i| i.time) * 1.6) as i64 % 2 == 0;
                                dot(ui, if on { RED } else { Color32::from_rgb(120, 40, 40) });
                                ui.label("Aufnahme läuft");
                            }
                            Some(RecState::Starting) | Some(RecState::Saving) => {
                                dot(ui, AMBER);
                                ui.label("Bitte warten ...");
                            }
                            Some(RecState::Done(_)) => {
                                dot(ui, GREEN);
                                ui.label("Aufnahme gespeichert");
                            }
                            Some(RecState::Failed(_)) => {
                                dot(ui, RED);
                                ui.label("Fehler");
                            }
                            None => {
                                dot(ui, if ff_ready { GREEN } else { MUTED });
                                ui.label(RichText::new(if ff_ready { "Bereit" } else { "Warte auf ffmpeg" }).color(MUTED));
                            }
                        }
                    });
                    if let Some(RecState::Done(p)) = &st {
                        ui.label(RichText::new(p.display().to_string()).size(12.5).color(MUTED));
                        ui.horizontal(|ui| {
                            if ui.button("Datei anzeigen").clicked() {
                                open_path = Some((p.clone(), true));
                            }
                            if ui.button("Ordner öffnen").clicked() {
                                open_path = Some((p.parent().unwrap_or(p).to_path_buf(), false));
                            }
                        });
                    }
                    if let Some(RecState::Failed(e)) = &st {
                        ui.colored_label(RED, e);
                    }
                    if !self.msg.is_empty() {
                        ui.colored_label(RED, &self.msg);
                    }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(20.0, 14.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(APP_TITLE).size(26.0).strong());
                            ui.label(RichText::new("Bildschirm und Ton aufnehmen").color(MUTED));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            let chip = ui
                                .add(
                                    egui::Button::new(RichText::new(format!("GitHub  v{VERSION}")).size(12.5).color(MUTED))
                                        .fill(CARD)
                                        .rounding(999.0),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(REPO_URL);
                            if chip.clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(REPO_URL));
                            }
                        });
                    });
                    ui.add_space(2.0);

                    // Während des Downloads ist alles gesperrt
                    if !ff_ready && !matches!(ffst, FfState::Checking) {
                        ffmpeg_card(ui, &ffst, &mut retry_ff);
                    }

                    ui.add_enabled_ui(!active && ff_ready, |ui| {
                        card(ui, "Aufnahmebereich", |ui| {
                            segmented(
                                ui,
                                &mut self.s.mode,
                                &[
                                    (Mode::Desktop, "Desktop"),
                                    (Mode::Monitor, "Monitor"),
                                    (Mode::Window, "Fenster"),
                                    (Mode::Region, "Bereich"),
                                ],
                            );
                            match self.s.mode {
                                Mode::Desktop => {
                                    ui.label(RichText::new("Alle Monitore zusammen.").color(MUTED));
                                }
                                Mode::Monitor => {
                                    let label = self
                                        .monitors
                                        .get(self.s.monitor)
                                        .map(|m| m.label(self.s.monitor))
                                        .unwrap_or_else(|| "-".into());
                                    egui::ComboBox::new("mon", "")
                                        .selected_text(label)
                                        .width(ui.available_width() - 24.0)
                                        .show_ui(ui, |ui| {
                                            for (i, m) in self.monitors.iter().enumerate() {
                                                ui.selectable_value(&mut self.s.monitor, i, m.label(i));
                                            }
                                        });
                                }
                                Mode::Window => {
                                    let label = if self.s.window_title.is_empty() {
                                        "Fenster wählen ...".to_string()
                                    } else {
                                        self.s.window_title.clone()
                                    };
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("Aktualisieren").clicked() {
                                            refresh_win = true;
                                        }
                                        egui::ComboBox::new("win", "")
                                            .selected_text(label)
                                            .width((ui.available_width() - 24.0).max(80.0))
                                            .show_ui(ui, |ui| {
                                                for t in &self.windows {
                                                    ui.selectable_value(&mut self.s.window_title, t.clone(), t);
                                                }
                                            });
                                    });
                                    ui.label(
                                        RichText::new("Aufgenommen wird der Bildschirmbereich des Fensters. Fenster nicht verschieben oder verdecken.")
                                            .size(12.5)
                                            .color(MUTED),
                                    );
                                }
                                Mode::Region => {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("X");
                                        ui.add(egui::DragValue::new(&mut self.s.region.x));
                                        ui.label("Y");
                                        ui.add(egui::DragValue::new(&mut self.s.region.y));
                                        ui.label("Breite");
                                        ui.add(egui::DragValue::new(&mut self.s.region.w));
                                        ui.label("Höhe");
                                        ui.add(egui::DragValue::new(&mut self.s.region.h));
                                    });
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(!self.shot_pending && ff_ready, egui::Button::new("Bereich per Maus wählen ..."))
                                            .clicked()
                                        {
                                            grab = true;
                                        }
                                        if self.shot_pending {
                                            ui.label(RichText::new("Screenshot wird erstellt ...").color(MUTED));
                                        }
                                    });
                                }
                            }
                        });

                        card(ui, "Audioquellen", |ui| {
                            if self.devices.is_empty() {
                                ui.label("Keine Audiogeräte gefunden.");
                            }
                            for (id, label) in &self.devices {
                                let mut on = self.s.audio.contains(id);
                                if ui.checkbox(&mut on, label).changed() {
                                    if on {
                                        self.s.audio.push(id.clone());
                                    } else {
                                        self.s.audio.retain(|x| x != id);
                                    }
                                }
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button("Liste aktualisieren").clicked() {
                                    refresh_audio = true;
                                }
                                ui.label(RichText::new("Mehrere Quellen werden gemischt.").size(12.5).color(MUTED));
                            });
                        });

                        card(ui, "Speicherort", |ui| {
                            let row_width = ui.available_width();
                            ui.allocate_ui_with_layout(
                                egui::vec2(row_width, 30.0),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Durchsuchen ...").clicked() {
                                        browse = true;
                                    }
                                    ui.add_sized(
                                        [ui.available_width(), 30.0],
                                        egui::TextEdit::singleline(&mut self.s.out_dir)
                                            .hint_text(config::default_out_dir().display().to_string())
                                            .margin(egui::vec2(10.0, 6.0)),
                                    );
                                },
                            );
                            ui.label(RichText::new("Leer = Ordner \"videos\" neben der exe.").size(12.5).color(MUTED));
                        });

                        card(ui, "Optionen", |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Bildrate");
                                egui::ComboBox::new("fps", "")
                                    .selected_text(format!("{} fps", self.s.fps))
                                    .show_ui(ui, |ui| {
                                        for f in [15u32, 24, 30, 60] {
                                            ui.selectable_value(&mut self.s.fps, f, format!("{f} fps"));
                                        }
                                    });
                            });
                            ui.checkbox(&mut self.s.minimize, "Fenster beim Start minimieren");
                        });
                    });

                });
            });

        if retry_ff {
            ffmpeg::ensure(self.ff.clone(), ctx.clone());
        }
        if refresh_win {
            self.windows = winapi::list_windows();
        }
        if refresh_audio {
            self.devices = list_audio();
        }
        if browse {
            let mut d = rfd::FileDialog::new();
            let cur = self.s.resolved_out_dir();
            if cur.exists() {
                d = d.set_directory(cur);
            }
            if let Some(p) = d.pick_folder() {
                self.s.out_dir = p.display().to_string();
            }
        }
        if grab {
            self.msg.clear();
            self.start_grab(ctx);
        }
        if let Some((p, select)) = open_path {
            let mut c = std::process::Command::new("explorer");
            if select {
                c.arg(format!("/select,{}", p.display()));
            } else {
                c.arg(p);
            }
            let _ = c.spawn();
        }
        if stop {
            if let Some(h) = &self.rec {
                h.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        if start {
            self.msg.clear();
            self.s.save();
            self.rec = Some(rec::start(self.s.clone(), ctx.clone()));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
    }
}

fn main() -> eframe::Result<()> {
    winapi::set_dpi_aware();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(SMALL)
            .with_min_inner_size([520.0, 520.0])
            .with_title(APP_TITLE),
        ..Default::default()
    };
    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}
