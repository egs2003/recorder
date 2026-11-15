#![windows_subsystem = "windows"]

mod config;
mod indicator;
mod rec;
mod winapi;

use config::{Mode, Settings, SourceId};
use cpal::traits::{DeviceTrait, HostTrait};
use eframe::egui;
use rec::RecState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const APP_TITLE: &str = "Bildschirm-Recorder";
const SMALL: [f32; 2] = [540.0, 760.0];
const BIG: [f32; 2] = [1150.0, 760.0];

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
                v.push((SourceId { output: true, name: n.clone() }, format!("Systemton: {n}")));
            }
        }
    }
    if let Ok(it) = host.input_devices() {
        for d in it {
            if let Ok(n) = d.name() {
                v.push((SourceId { output: false, name: n.clone() }, format!("Eingang: {n}")));
            }
        }
    }
    v
}

impl App {
    fn new() -> Self {
        let mut s = Settings::load();
        if !s.audio_initialized {
            if let Some(n) = cpal::default_host().default_output_device().and_then(|d| d.name().ok()) {
                s.audio.push(SourceId { output: true, name: n });
            }
            s.audio_initialized = true;
        }
        App {
            s,
            monitors: winapi::list_monitors(),
            windows: winapi::list_windows(),
            devices: list_audio(),
            rec: None,
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
        egui::CentralPanel::default().show(ctx, |ui| {
            let p = self.picker.as_mut().unwrap();
            ui.label("Mit gedrückter Maustaste einen Bereich aufziehen.");
            let tsize = p.tex.size_vec2();
            let avail = ui.available_size() - egui::vec2(0.0, 50.0);
            let scale = (avail.x / tsize.x).min(avail.y / tsize.y).max(0.05);
            let (resp, painter) = ui.allocate_painter(tsize * scale, egui::Sense::drag());
            let rect = resp.rect;
            painter.image(
                p.tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
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
                painter.rect_filled(sr, 0.0, egui::Color32::from_rgba_unmultiplied(255, 0, 0, 40));
                painter.rect_stroke(sr, 0.0, egui::Stroke::new(2.0, egui::Color32::RED));
            }
            ui.horizontal(|ui| {
                let ok = matches!((p.a, p.b), (Some(a), Some(b)) if (a.x - b.x).abs() > 0.005 && (a.y - b.y).abs() > 0.005);
                if ui.add_enabled(ok, egui::Button::new("Übernehmen")).clicked() {
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

        let mut start = false;
        let mut stop = false;
        let mut grab = false;
        let mut browse = false;
        let mut refresh_win = false;
        let mut refresh_audio = false;
        let mut open_path: Option<(PathBuf, bool)> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading(APP_TITLE);
                ui.add_space(6.0);

                ui.add_enabled_ui(!active, |ui| {
                    ui.strong("Aufnahmebereich");
                    ui.radio_value(&mut self.s.mode, Mode::Desktop, "Gesamter Desktop (alle Monitore)");
                    ui.radio_value(&mut self.s.mode, Mode::Monitor, "Ein Monitor");
                    if self.s.mode == Mode::Monitor {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let label = self
                                .monitors
                                .get(self.s.monitor)
                                .map(|m| m.label(self.s.monitor))
                                .unwrap_or_else(|| "-".into());
                            egui::ComboBox::new("mon", "").selected_text(label).width(340.0).show_ui(ui, |ui| {
                                for (i, m) in self.monitors.iter().enumerate() {
                                    ui.selectable_value(&mut self.s.monitor, i, m.label(i));
                                }
                            });
                        });
                    }
                    ui.radio_value(&mut self.s.mode, Mode::Window, "Ein Fenster");
                    if self.s.mode == Mode::Window {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let label = if self.s.window_title.is_empty() {
                                "(Fenster wählen)".to_string()
                            } else {
                                self.s.window_title.clone()
                            };
                            egui::ComboBox::new("win", "").selected_text(label).width(340.0).show_ui(ui, |ui| {
                                for t in &self.windows {
                                    ui.selectable_value(&mut self.s.window_title, t.clone(), t);
                                }
                            });
                            if ui.button("Aktualisieren").clicked() {
                                refresh_win = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.weak("Wird als Bildschirmbereich aufgenommen: Fenster nicht verschieben oder verdecken.");
                        });
                    }
                    ui.radio_value(&mut self.s.mode, Mode::Region, "Fester Bereich");
                    if self.s.mode == Mode::Region {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
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
                            ui.add_space(20.0);
                            if ui.add_enabled(!self.shot_pending, egui::Button::new("Bereich per Maus wählen ...")).clicked() {
                                grab = true;
                            }
                            if self.shot_pending {
                                ui.label("Screenshot wird erstellt ...");
                            }
                        });
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.strong("Audioquellen");
                        if ui.small_button("Aktualisieren").clicked() {
                            refresh_audio = true;
                        }
                    });
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
                    ui.weak("Mehrere Quellen werden gemischt. Keine Auswahl = ohne Ton.");

                    ui.add_space(10.0);
                    ui.strong("Speicherort");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.s.out_dir)
                                .hint_text(config::default_out_dir().display().to_string())
                                .desired_width(340.0),
                        );
                        if ui.button("Durchsuchen ...").clicked() {
                            browse = true;
                        }
                    });
                    ui.weak("Leer lassen = Ordner \"videos\" neben der exe.");

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Bildrate:");
                        egui::ComboBox::new("fps", "").selected_text(format!("{} fps", self.s.fps)).show_ui(ui, |ui| {
                            for f in [15u32, 24, 30, 60] {
                                ui.selectable_value(&mut self.s.fps, f, format!("{f} fps"));
                            }
                        });
                    });
                    ui.checkbox(&mut self.s.minimize, "Fenster beim Start minimieren");
                });

                ui.add_space(14.0);
                let w = ui.available_width();
                match &st {
                    Some(RecState::Starting) => {
                        ui.add_enabled(false, egui::Button::new("Starte ...").min_size(egui::vec2(w, 44.0)));
                    }
                    Some(RecState::Saving) => {
                        ui.add_enabled(false, egui::Button::new("Speichere ...").min_size(egui::vec2(w, 44.0)));
                    }
                    Some(RecState::Recording(t)) => {
                        let secs = t.elapsed().as_secs();
                        let label = format!("Stopp   {:02}:{:02}", secs / 60, secs % 60);
                        if ui.add(egui::Button::new(label).min_size(egui::vec2(w, 44.0))).clicked() {
                            stop = true;
                        }
                    }
                    _ => {
                        if ui.add(egui::Button::new("Aufnahme starten").min_size(egui::vec2(w, 44.0))).clicked() {
                            start = true;
                        }
                    }
                }

                ui.add_space(8.0);
                if let Some(RecState::Done(p)) = &st {
                    ui.label(format!("Gespeichert: {}", p.display()));
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
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Fehler: {e}"));
                }
                if !self.msg.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.msg);
                }
            });
        });

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
            .with_min_inner_size([480.0, 520.0])
            .with_title(APP_TITLE),
        ..Default::default()
    };
    eframe::run_native(APP_TITLE, options, Box::new(|_cc| Ok(Box::new(App::new()))))
}
