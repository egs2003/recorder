#![windows_subsystem = "windows"]

mod config;
mod ffmpeg;
mod identify;
mod indicator;
mod plan;
mod rec;
mod update;
mod winapi;

use config::{Mode, Settings, SourceId};
use cpal::traits::{DeviceTrait, HostTrait};
use eframe::egui::{self, Color32, RichText};
use ffmpeg::FfState;
use rec::RecState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use update::UpState;

pub const APP_TITLE: &str = "Bildschirm-Recorder";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/egs2003/recorder";
const SMALL: [f32; 2] = [600.0, 800.0];
const BIG: [f32; 2] = [1150.0, 780.0];

// Farben: je nach Systemdesign (dunkel/hell). `pal()` liefert die Palette des aktuellen Themes,
// `update` setzt sie zu Beginn jedes Frames anhand von `ctx.theme()`.
#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    card: Color32,
    border: Color32,
    widget: Color32,
    widget_hover: Color32,
    accent: Color32,
    /// Hintergrund für ausgewählte Elemente (Segmente, Textmarkierung)
    sel_bg: Color32,
    red: Color32,
    green: Color32,
    amber: Color32,
    muted: Color32,
    info_bg: Color32,
    info_text: Color32,
    err_bg: Color32,
    err_text: Color32,
    /// "Aus"-Phase des blinkenden Aufnahmepunkts
    rec_dim: Color32,
    input_bg: Color32,
}

const DARK: Pal = Pal {
    bg: Color32::from_rgb(20, 22, 27),
    card: Color32::from_rgb(30, 33, 40),
    border: Color32::from_rgb(46, 50, 60),
    widget: Color32::from_rgb(41, 45, 55),
    widget_hover: Color32::from_rgb(52, 57, 70),
    accent: Color32::from_rgb(86, 132, 255),
    sel_bg: Color32::from_rgb(86, 132, 255),
    red: Color32::from_rgb(232, 64, 64),
    green: Color32::from_rgb(70, 190, 120),
    amber: Color32::from_rgb(235, 170, 50),
    muted: Color32::from_rgb(140, 147, 163),
    info_bg: Color32::from_rgb(26, 35, 64),
    info_text: Color32::from_rgb(169, 184, 230),
    err_bg: Color32::from_rgb(52, 26, 30),
    err_text: Color32::from_rgb(240, 170, 170),
    rec_dim: Color32::from_rgb(120, 40, 40),
    input_bg: Color32::from_rgb(24, 26, 32),
};

const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(243, 244, 247),
    card: Color32::from_rgb(255, 255, 255),
    border: Color32::from_rgb(218, 222, 230),
    widget: Color32::from_rgb(234, 237, 243),
    widget_hover: Color32::from_rgb(222, 227, 237),
    accent: Color32::from_rgb(55, 100, 235),
    sel_bg: Color32::from_rgb(205, 220, 255),
    red: Color32::from_rgb(214, 48, 48),
    green: Color32::from_rgb(34, 158, 90),
    amber: Color32::from_rgb(214, 140, 20),
    muted: Color32::from_rgb(98, 106, 123),
    info_bg: Color32::from_rgb(228, 237, 255),
    info_text: Color32::from_rgb(45, 68, 130),
    err_bg: Color32::from_rgb(253, 232, 232),
    err_text: Color32::from_rgb(170, 35, 35),
    rec_dim: Color32::from_rgb(240, 160, 160),
    input_bg: Color32::from_rgb(250, 251, 253),
};

static IS_LIGHT: AtomicBool = AtomicBool::new(false);

fn pal() -> Pal {
    if IS_LIGHT.load(AtomicOrdering::Relaxed) {
        LIGHT
    } else {
        DARK
    }
}

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
    upd: Arc<Mutex<UpState>>,
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

/// Folgt der Systemeinstellung (dunkel/hell) und legt für beide Themes einen eigenen Style an.
fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    ctx.set_style_of(egui::Theme::Dark, build_style(&DARK, false));
    ctx.set_style_of(egui::Theme::Light, build_style(&LIGHT, true));
}

fn build_style(p: &Pal, light: bool) -> egui::Style {
    use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
    let mut style = egui::Style::default();
    style.text_styles.insert(TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Button, FontId::new(15.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small, FontId::new(12.5, FontFamily::Proportional));
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 26.0;

    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = p.input_bg;
    v.selection.bg_fill = p.sel_bg;
    // egui nimmt `selection.stroke` als Schriftfarbe ausgewählter Einträge (z. B. im Dropdown) und
    // `selection.bg_fill` als deren Hintergrund. Beide dürfen nicht dieselbe Farbe haben.
    let sel_text = if light { Color32::from_rgb(25, 55, 150) } else { Color32::from_rgb(240, 244, 255) };
    v.selection.stroke = Stroke::new(1.0_f32, sel_text);
    v.hyperlink_color = p.accent;
    let r = Rounding::same(9.0);
    v.widgets.noninteractive.rounding = r;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.rounding = r;
    v.widgets.inactive.weak_bg_fill = p.widget;
    v.widgets.inactive.bg_fill = p.widget;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.hovered.rounding = r;
    v.widgets.hovered.weak_bg_fill = p.widget_hover;
    v.widgets.hovered.bg_fill = p.widget_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.rounding = r;
    // Achtung: `RichText::strong()` nimmt die Farbe von widgets.active.fg_stroke. Diese Schriftfarbe
    // deshalb nie ändern (hell: dunkel, dunkel: hell), sondern den Hintergrund anpassen: im hellen
    // Design bekommt der gedrückte Zustand den hellblauen statt des kräftigen Akzent-Hintergrunds.
    let pressed = if light { p.sel_bg } else { p.accent };
    v.widgets.active.weak_bg_fill = pressed;
    v.widgets.active.bg_fill = pressed;
    v.widgets.open.rounding = r;
    v.widgets.open.weak_bg_fill = p.widget_hover;
    v.widgets.open.bg_fill = p.widget_hover;
    style.visuals = v;
    style
}

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(pal().card)
        .stroke(egui::Stroke::new(1.0_f32, pal().border))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title.to_uppercase()).size(12.0).color(pal().muted).strong());
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

/// Hinweis zum Update: Rückfrage bei neuer Version, Fortschritt, Fehler. Setzt `install`/`dismiss`.
fn update_card(ui: &mut egui::Ui, st: &UpState, can_install: bool, install: &mut bool, dismiss: &mut bool) {
    let failed = matches!(st, UpState::Failed(_));
    if !failed && !matches!(st, UpState::Available(_)) && !st.busy() {
        return;
    }
    let (fill, accent) = if failed { (pal().err_bg, pal().red) } else { (pal().info_bg, pal().accent) };
    let text = if failed { pal().err_text } else { pal().info_text };
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, accent))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match st {
                UpState::Available(r) => {
                    ui.label(RichText::new(format!("Neue Version {} verfügbar", r.version)).size(16.0).strong());
                    ui.label(
                        RichText::new(format!(
                            "Installiert ist Version {}. Die neue Version wird von GitHub heruntergeladen, danach startet die App neu.",
                            update::CURRENT
                        ))
                        .size(12.5)
                        .color(text),
                    );
                    let notes: String = r.notes.lines().take(4).collect::<Vec<_>>().join("\n");
                    let notes: String = notes.chars().take(300).collect();
                    if !notes.trim().is_empty() {
                        ui.label(RichText::new(notes).size(12.5).color(pal().muted));
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(can_install, egui::Button::new("Jetzt aktualisieren")).clicked() {
                            *install = true;
                        }
                        if ui.button("Später").clicked() {
                            *dismiss = true;
                        }
                        if !can_install {
                            ui.label(RichText::new("Während der Aufnahme nicht möglich.").size(12.5).color(pal().muted));
                        }
                    });
                }
                UpState::Downloading { done, total } => {
                    ui.label(RichText::new("Update wird heruntergeladen").size(16.0).strong());
                    if *total > 0 {
                        ui.add(egui::ProgressBar::new((*done as f32 / *total as f32).min(1.0)).desired_height(10.0).fill(pal().accent));
                        ui.label(RichText::new(format!("{} von {}", mb(*done), mb(*total))).size(13.0));
                    } else {
                        ui.add(egui::ProgressBar::new(0.0).desired_height(10.0).fill(pal().accent).animate(true));
                        ui.label(RichText::new(format!("{} heruntergeladen", mb(*done))).size(13.0));
                    }
                }
                UpState::Installing | UpState::ReadyToRestart | UpState::Restarting => {
                    ui.label(RichText::new("Update wird installiert, die App startet gleich neu ...").size(16.0).strong());
                    ui.add(egui::ProgressBar::new(1.0).desired_height(10.0).fill(pal().accent).animate(true));
                }
                UpState::Failed(e) => {
                    ui.label(RichText::new("Update nicht möglich").size(16.0).strong());
                    ui.label(RichText::new(e).size(13.0).color(text));
                    if ui.button("Schließen").clicked() {
                        *dismiss = true;
                    }
                }
                _ => {}
            }
        });
    ui.add_space(8.0);
}

fn ffmpeg_card(ui: &mut egui::Ui, st: &FfState, retry: &mut bool) {
    let failed = matches!(st, FfState::Failed(_));
    let (fill, accent) = if failed { (pal().err_bg, pal().red) } else { (pal().info_bg, pal().accent) };
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, accent))
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
                        let s = egui::Stroke::new(2.2_f32, Color32::WHITE);
                        p.line_segment([c + egui::vec2(0.0, -8.0), c + egui::vec2(0.0, 6.0)], s);
                        p.line_segment([c + egui::vec2(-5.0, 1.0), c + egui::vec2(0.0, 7.0)], s);
                        p.line_segment([c + egui::vec2(5.0, 1.0), c + egui::vec2(0.0, 7.0)], s);
                    }
                }
                ui.vertical(|ui| {
                    let title = if failed { "ffmpeg konnte nicht eingerichtet werden" } else { "ffmpeg wird eingerichtet" };
                    ui.label(RichText::new(title).size(16.0).strong());
                    if !failed {
                        ui.label(RichText::new("Einmalig beim ersten Start. Danach ist die App bereit.").size(12.5).color(pal().info_text));
                    }
                });
            });
            match st {
                FfState::Downloading { done, total } => {
                    if *total > 0 {
                        ui.add(egui::ProgressBar::new(*done as f32 / *total as f32).desired_height(10.0).fill(pal().accent));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} von {}", mb(*done), mb(*total))).size(13.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("{} %", (*done * 100 / *total).min(100))).size(13.0));
                            });
                        });
                    } else {
                        ui.add(egui::ProgressBar::new(0.0).desired_height(10.0).fill(pal().accent).animate(true));
                        ui.label(RichText::new(format!("{} heruntergeladen", mb(*done))).size(13.0));
                    }
                }
                FfState::Extracting => {
                    ui.add(egui::ProgressBar::new(1.0).desired_height(10.0).fill(pal().accent).animate(true));
                    ui.label(RichText::new("Wird entpackt ...").size(13.0));
                }
                FfState::Failed(e) => {
                    ui.label(RichText::new(e).size(13.0).color(pal().err_text));
                    if ui.button("Erneut versuchen").clicked() {
                        *retry = true;
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Aufnahme und Einstellungen sind bis zum Abschluss gesperrt.").size(12.5).color(if failed { pal().err_text } else { pal().info_text }));
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
        // Update-Prüfung beim Start: ohne Nachfrage und still; gefragt wird erst, wenn es etwas gibt
        update::cleanup();
        let upd = Arc::new(Mutex::new(UpState::Idle));
        update::check(upd.clone(), ctx.clone(), false);
        App {
            upd,
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
            .frame(egui::Frame::none().fill(pal().bg).inner_margin(egui::Margin::same(16.0)))
            .show(ctx, |ui| {
                let p = self.picker.as_mut().unwrap();
                ui.label(RichText::new("Bereich auswählen").size(20.0).strong());
                ui.label(RichText::new("Mit gedrückter Maustaste einen Bereich aufziehen.").color(pal().muted));
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
                    painter.rect_stroke(sr, 0.0, egui::Stroke::new(2.0_f32, pal().accent));
                }
                ui.horizontal(|ui| {
                    let ok = matches!((p.a, p.b), (Some(a), Some(b)) if (a.x - b.x).abs() > 0.005 && (a.y - b.y).abs() > 0.005);
                    let btn = egui::Button::new(RichText::new("Übernehmen").strong().color(Color32::WHITE)).fill(pal().accent);
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
        // Palette des aktuellen Systemdesigns (dunkel/hell) für diesen Frame
        IS_LIGHT.store(ctx.theme() == egui::Theme::Light, AtomicOrdering::Relaxed);

        // Update ist installiert: Einstellungen sichern, neue Version starten, diese beenden
        if matches!(*self.upd.lock().unwrap(), UpState::ReadyToRestart) {
            self.s.save();
            match update::restart() {
                Ok(()) => {
                    // Sofort merken: bis das Fenster wirklich zu ist, laufen evtl. noch Frames, und die
                    // dürfen die App nicht noch einmal starten
                    *self.upd.lock().unwrap() = UpState::Restarting;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                Err(e) => *self.upd.lock().unwrap() = UpState::Failed(format!("{e:#}")),
            }
        }
        if matches!(*self.upd.lock().unwrap(), UpState::Restarting) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

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
        let rec_info = self.rec.as_ref().map(|h| h.info.lock().unwrap().clone()).unwrap_or_default();
        let active = matches!(st, Some(RecState::Starting) | Some(RecState::Recording(_)) | Some(RecState::Saving));
        if active {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let ffst = self.ff.lock().unwrap().clone();
        let ff_ready = matches!(ffst, FfState::Ready);
        let upst = self.upd.lock().unwrap().clone();
        // Während Download/Installation darf nichts aufgenommen werden, der Neustart würde es beenden
        let updating = upst.busy();

        let mut install_upd = false;
        let mut dismiss_upd = false;
        let mut check_upd = false;
        let mut start = false;
        let mut stop = false;
        let mut grab = false;
        let mut browse = false;
        let mut refresh_win = false;
        let mut refresh_audio = false;
        let mut retry_ff = false;
        let mut identify_monitors = false;
        let mut refresh_mon = false;
        let mut open_path: Option<(PathBuf, bool)> = None;

        egui::TopBottomPanel::bottom("bottom")
            .frame(egui::Frame::none().fill(pal().bg).inner_margin(egui::Margin::symmetric(20.0, 12.0)))
            .show(ctx, |ui| {
                    let w = ui.available_width();
                    let big = |text: String, fill: Color32| {
                        // auf dem neutralen (deaktivierten) Hintergrund keine weiße Schrift, sonst im hellen Design unlesbar
                        let txt = if fill == pal().widget { pal().muted } else { Color32::WHITE };
                        egui::Button::new(RichText::new(text).size(18.0).strong().color(txt))
                            .fill(fill)
                            .rounding(12.0)
                            .min_size(egui::vec2(w, 52.0))
                    };
                    match &st {
                        Some(RecState::Starting) => {
                            ui.add_enabled(false, big("Starte ...".into(), pal().amber));
                        }
                        Some(RecState::Saving) => {
                            ui.add_enabled(false, big("Speichere ...".into(), pal().amber));
                        }
                        Some(RecState::Recording(t)) => {
                            let secs = t.elapsed().as_secs();
                            let label = format!("Aufnahme beenden    {:02}:{:02}", secs / 60, secs % 60);
                            if ui.add(big(label, pal().red)).clicked() {
                                stop = true;
                            }
                        }
                        _ => {
                            if ui
                                .add_enabled(
                                    ff_ready && !updating,
                                    big("Aufnahme starten".into(), if ff_ready && !updating { pal().red } else { pal().widget }),
                                )
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
                                dot(ui, if on { pal().red } else { pal().rec_dim });
                                ui.label("Aufnahme läuft");
                                if !rec_info.is_empty() {
                                    ui.label(RichText::new(format!("({rec_info})")).size(12.5).color(pal().muted));
                                }
                            }
                            Some(RecState::Starting) | Some(RecState::Saving) => {
                                dot(ui, pal().amber);
                                ui.label("Bitte warten ...");
                            }
                            Some(RecState::Done(_)) => {
                                dot(ui, pal().green);
                                ui.label("Aufnahme gespeichert");
                            }
                            Some(RecState::Failed(_)) => {
                                dot(ui, pal().red);
                                ui.label("Fehler");
                            }
                            None => {
                                dot(ui, if ff_ready { pal().green } else { pal().muted });
                                ui.label(RichText::new(if ff_ready { "Bereit" } else { "Warte auf ffmpeg" }).color(pal().muted));
                            }
                        }
                    });
                    if let Some(RecState::Done(p)) = &st {
                        ui.label(RichText::new(p.display().to_string()).size(12.5).color(pal().muted));
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
                        ui.colored_label(pal().red, e);
                    }
                    if !self.msg.is_empty() {
                        ui.colored_label(pal().red, &self.msg);
                    }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal().bg).inner_margin(egui::Margin::symmetric(20.0, 14.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(APP_TITLE).size(26.0).strong());
                            ui.label(RichText::new("Bildschirm und Ton aufnehmen").color(pal().muted));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            let chip = ui
                                .add(
                                    egui::Button::new(RichText::new(format!("GitHub  v{VERSION}")).size(12.5).color(pal().muted))
                                        .fill(pal().card)
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

                    update_card(ui, &upst, !active, &mut install_upd, &mut dismiss_upd);

                    // Während des Downloads ist alles gesperrt
                    if !ff_ready && !matches!(ffst, FfState::Checking) {
                        ffmpeg_card(ui, &ffst, &mut retry_ff);
                    }

                    ui.add_enabled_ui(!active && ff_ready && !updating, |ui| {
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
                                    ui.label(RichText::new("Alle Monitore zusammen.").color(pal().muted));
                                }
                                Mode::Monitor => {
                                    let label = self
                                        .monitors
                                        .get(self.s.monitor)
                                        .map(|m| m.label(self.s.monitor))
                                        .unwrap_or_else(|| "-".into());
                                    let before = self.s.monitor;
                                    // Auswahlliste über die volle Breite, die Buttons darunter. Zwei Buttons plus
                                    // Liste in einer Zeile wurden breiter als die Karte, und das Fenster wuchs.
                                    egui::ComboBox::new("mon", "")
                                        .selected_text(label)
                                        .width((ui.available_width() - 24.0).max(80.0))
                                        .show_ui(ui, |ui| {
                                            for (i, m) in self.monitors.iter().enumerate() {
                                                ui.selectable_value(&mut self.s.monitor, i, m.label(i));
                                            }
                                        });
                                    ui.horizontal(|ui| {
                                        if ui.button("Aktualisieren").clicked() {
                                            refresh_mon = true;
                                        }
                                        if ui.button("Identifizieren").clicked() {
                                            identify_monitors = true;
                                        }
                                    });
                                    // Wer einen anderen Monitor wählt, sieht sofort, welcher es ist
                                    if self.s.monitor != before {
                                        identify_monitors = true;
                                    }
                                    ui.label(
                                        RichText::new("Nummer wie in der Liste, gewählter Monitor ist blau.")
                                            .size(12.5)
                                            .color(pal().muted),
                                    );
                                }
                                Mode::Window => {
                                    let label = if self.s.window_title.is_empty() {
                                        "Fenster wählen ...".to_string()
                                    } else {
                                        self.s.window_title.clone()
                                    };
                                    let row_width = ui.available_width();
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(row_width, 30.0),
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
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
                                        },
                                    );
                                    ui.label(
                                        RichText::new("Aufgenommen wird der Bildschirmbereich des Fensters. Fenster nicht verschieben oder verdecken.")
                                            .size(12.5)
                                            .color(pal().muted),
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
                                            ui.label(RichText::new("Screenshot wird erstellt ...").color(pal().muted));
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
                                ui.label(RichText::new("Mehrere Quellen werden gemischt.").size(12.5).color(pal().muted));
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
                            ui.label(RichText::new("Leer = Ordner \"videos\" neben der exe.").size(12.5).color(pal().muted));
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
                            ui.checkbox(&mut self.s.hw_accel, "Hardwarebeschleunigung nutzen");
                            let hint = match ffmpeg::caps_if_ready() {
                                Some(c) => c.summary(),
                                None => "Hardware wird geprüft ...".to_string(),
                            };
                            ui.label(RichText::new(hint).size(12.5).color(pal().muted));
                            ui.label(
                                RichText::new("GPU-Aufnahme (ddagrab) wird nur bei genau einem angeschlossenen Monitor verwendet, sonst GDI-Aufnahme mit Hardware-Encoder.")
                                    .size(12.5)
                                    .color(pal().muted),
                            );
                            ui.horizontal(|ui| {
                                let searching = matches!(upst, UpState::Checking);
                                if ui.add_enabled(!searching, egui::Button::new("Nach Updates suchen")).clicked() {
                                    check_upd = true;
                                }
                                let status = match &upst {
                                    UpState::Checking => "Suche läuft ...".to_string(),
                                    UpState::UpToDate => format!("Version {VERSION} ist aktuell."),
                                    _ => format!("Version {VERSION}"),
                                };
                                ui.label(RichText::new(status).size(12.5).color(pal().muted));
                            });
                        });
                    });

                });
            });

        if install_upd {
            let rel = match &*self.upd.lock().unwrap() {
                UpState::Available(r) => Some(r.clone()),
                _ => None,
            };
            if let Some(r) = rel {
                self.s.save();
                update::install(self.upd.clone(), ctx.clone(), r);
            }
        }
        if dismiss_upd {
            *self.upd.lock().unwrap() = UpState::Idle;
        }
        if check_upd {
            update::check(self.upd.clone(), ctx.clone(), true);
        }
        if retry_ff {
            ffmpeg::ensure(self.ff.clone(), ctx.clone());
        }
        if refresh_win {
            self.windows = winapi::list_windows();
        }
        if refresh_audio {
            self.devices = list_audio();
        }
        if refresh_mon || identify_monitors {
            // Monitorliste neu einlesen (Monitor an-/abgesteckt?)
            self.monitors = winapi::list_monitors();
            self.s.monitor = self.s.monitor.min(self.monitors.len().saturating_sub(1));
        }
        if identify_monitors {
            let rects: Vec<_> = self.monitors.iter().map(|m| (m.x, m.y, m.w, m.h)).collect();
            identify::show(&rects, self.s.monitor);
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

/// Fenster-Icon (128x128 RGBA, aus assets/icon.rgba)
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

fn main() -> eframe::Result<()> {
    winapi::set_dpi_aware();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(SMALL)
            .with_min_inner_size([520.0, 520.0])
            .with_title(APP_TITLE)
            .with_icon(window_icon()),
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
