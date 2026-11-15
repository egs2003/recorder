//! Zusammenbau der ffmpeg-Argumente: Aufnahme-Backend x Encoder, inklusive der
//! Fallback-Reihenfolge. Dieses Modul hängt von nichts Windows-spezifischem ab und ist
//! deshalb getrennt testbar.
//!
//! Aufnahme-Backends:
//!   * `Ddagrab`  - Desktop Duplication (GPU). Liefert D3D11-Frames, die ohne Umweg über den
//!                  Arbeitsspeicher direkt an einen Hardware-Encoder gehen können.
//!                  Erfasst nur einen einzelnen Monitor.
//!   * `Gdigrab`  - klassische GDI-Aufnahme (CPU), funktioniert überall, auch über mehrere Monitore.
//!
//! Encoder: NVENC, QSV, AMF (Hardware) und libx264 (Software, immer als letzter Ausweg).

pub type Area = (i32, i32, i32, i32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoder {
    Nvenc,
    Qsv,
    Amf,
    X264,
}

impl Encoder {
    /// Reihenfolge, in der Hardware-Encoder bevorzugt werden.
    pub const HW: [Encoder; 3] = [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf];

    pub fn label(self) -> &'static str {
        match self {
            Encoder::Nvenc => "NVIDIA NVENC",
            Encoder::Qsv => "Intel QSV",
            Encoder::Amf => "AMD AMF",
            Encoder::X264 => "x264 (Software)",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capture {
    Ddagrab,
    Gdigrab,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    pub capture: Capture,
    pub encoder: Encoder,
}

impl Plan {
    pub fn label(&self) -> String {
        let cap = match self.capture {
            Capture::Ddagrab => "GPU-Aufnahme",
            Capture::Gdigrab => "GDI-Aufnahme",
        };
        format!("{cap} + {}", self.encoder.label())
    }
}

/// Was dieses System tatsächlich kann (durch Probeläufe ermittelt, nicht nur aus der
/// ffmpeg-Encoderliste gelesen - die zeigt nur, was einkompiliert ist).
#[derive(Clone, Debug, Default)]
pub struct Caps {
    /// ddagrab + Download in den Arbeitsspeicher + libx264 funktioniert
    pub ddagrab_x264: bool,
    /// ddagrab -> Hardware-Encoder ohne Umweg über den RAM funktioniert
    pub hw_path: Vec<Encoder>,
    /// Hardware-Encoder mit normalen Bildern aus dem RAM (für gdigrab) funktioniert
    pub sw_path: Vec<Encoder>,
}

impl Caps {
    pub fn summary(&self) -> String {
        let mut enc: Vec<&str> = Vec::new();
        for e in Encoder::HW {
            if self.hw_path.contains(&e) || self.sw_path.contains(&e) {
                enc.push(e.label());
            }
        }
        let gpu_capture = self.ddagrab_x264 || !self.hw_path.is_empty();
        match (gpu_capture, enc.is_empty()) {
            (false, true) => "Keine Hardwarebeschleunigung erkannt, es wird per Software kodiert.".to_string(),
            (true, true) => "Erkannt: GPU-Aufnahme (ddagrab), Software-Encoder.".to_string(),
            (false, false) => format!("Erkannt: {} (ohne GPU-Aufnahme).", enc.join(", ")),
            (true, false) => format!("Erkannt: GPU-Aufnahme (ddagrab), {}.", enc.join(", ")),
        }
    }
}

/// Was aufgenommen werden soll.
#[derive(Clone, Debug)]
pub struct Job {
    pub fps: u32,
    /// Aufnahmebereich in virtuellen Desktop-Koordinaten (x, y, Breite, Höhe)
    pub area: Area,
    /// `area` ist der gesamte virtuelle Desktop (gdigrab braucht dann keine Offsets)
    pub full_desktop: bool,
    /// Der einzige angeschlossene Monitor. `None` bei mehreren Monitoren: ddagrab erfasst nur
    /// einen Monitor, und die Zuordnung zum DXGI-Ausgabeindex ist nicht verlässlich.
    pub monitor: Option<Area>,
}

impl Job {
    fn ddagrab_possible(&self) -> bool {
        match self.monitor {
            Some(m) => {
                let a = self.area;
                a.0 >= m.0 && a.1 >= m.1 && a.0 + a.2 <= m.0 + m.2 && a.1 + a.3 <= m.1 + m.3
            }
            None => false,
        }
    }
}

/// Versuchsreihenfolge, vom schnellsten zum robustesten. Der letzte Eintrag (GDI + x264)
/// ist immer vorhanden.
pub fn plans(job: &Job, caps: &Caps, hw_allowed: bool) -> Vec<Plan> {
    let mut v = Vec::new();
    if hw_allowed {
        if job.ddagrab_possible() {
            for &e in &caps.hw_path {
                v.push(Plan { capture: Capture::Ddagrab, encoder: e });
            }
            if caps.ddagrab_x264 {
                v.push(Plan { capture: Capture::Ddagrab, encoder: Encoder::X264 });
            }
        }
        for &e in &caps.sw_path {
            v.push(Plan { capture: Capture::Gdigrab, encoder: e });
        }
    }
    v.push(Plan { capture: Capture::Gdigrab, encoder: Encoder::X264 });
    v
}

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// `gpu_frames`: die Bilder kommen als D3D11-Frames (Hardware-Encoder) bzw. nach
/// `hwdownload` (x264) von ddagrab. Sonst normale Bilder aus dem RAM (gdigrab).
fn encoder_args(enc: Encoder, gpu_frames: bool) -> Vec<String> {
    let mut a = match enc {
        Encoder::X264 => v(&["-c:v", "libx264", "-preset", "ultrafast", "-crf", "27"]),
        Encoder::Nvenc => v(&["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "vbr", "-cq", "25", "-b:v", "0"]),
        Encoder::Qsv => v(&["-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", "24"]),
        Encoder::Amf => v(&["-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "24", "-qp_p", "26"]),
    };
    let pix = match (enc, gpu_frames) {
        (Encoder::Qsv, false) => Some("nv12"),
        (Encoder::X264, _) | (_, false) => Some("yuv420p"),
        // GPU-Frames in den Hardware-Encoder: das Pixelformat bestimmt die Filterkette
        _ => None,
    };
    if let Some(p) = pix {
        a.extend(v(&["-pix_fmt", p]));
    }
    a
}

/// Filtergraph für ddagrab. `crop` = (x, y, Breite, Höhe) relativ zum Monitor.
fn ddagrab_graph(enc: Encoder, fps: u32, crop: Option<Area>) -> String {
    let mut g = format!("ddagrab=output_idx=0:framerate={fps}:draw_mouse=1");
    if let Some((x, y, w, h)) = crop {
        g += &format!(":video_size={w}x{h}:offset_x={x}:offset_y={y}");
    }
    g += match enc {
        Encoder::Qsv => ",hwmap=derive_device=qsv,format=qsv,vpp_qsv=format=nv12",
        Encoder::X264 => ",hwdownload,format=bgra,pad=ceil(iw/2)*2:ceil(ih/2)*2",
        Encoder::Nvenc | Encoder::Amf => "",
    };
    g + "[v]"
}

/// Eingabe, Filter und Encoder für einen Versuch (ohne Ausgabedatei).
pub fn video_args(p: Plan, job: &Job) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match p.capture {
        Capture::Gdigrab => {
            a.extend(v(&["-f", "gdigrab", "-framerate"]));
            a.push(job.fps.to_string());
            a.extend(v(&["-draw_mouse", "1"]));
            if !job.full_desktop {
                let r = job.area;
                a.extend(["-offset_x".to_string(), r.0.to_string()]);
                a.extend(["-offset_y".to_string(), r.1.to_string()]);
                a.extend(["-video_size".to_string(), format!("{}x{}", r.2, r.3)]);
            }
            a.extend(v(&["-i", "desktop", "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]));
            a.extend(encoder_args(p.encoder, false));
        }
        Capture::Ddagrab => {
            // offset/video_size sind bei ddagrab relativ zum Monitor
            let crop = match job.monitor {
                Some(m) if m != job.area => {
                    let r = job.area;
                    Some((r.0 - m.0, r.1 - m.1, r.2, r.3))
                }
                _ => None,
            };
            a.push("-filter_complex".to_string());
            a.push(ddagrab_graph(p.encoder, job.fps, crop));
            a.extend(v(&["-map", "[v]"]));
            a.extend(encoder_args(p.encoder, true));
        }
    }
    a
}

/// Probelauf: ddagrab (ganzer erster Monitor) -> Encoder, 5 Bilder ins Nichts.
pub fn probe_ddagrab_args(enc: Encoder) -> Vec<String> {
    let mut a = vec!["-filter_complex".to_string(), ddagrab_graph(enc, 30, None)];
    a.extend(v(&["-map", "[v]", "-frames:v", "5"]));
    a.extend(encoder_args(enc, true));
    a.extend(v(&["-f", "null", "-"]));
    a
}

/// Probelauf: Testbild aus dem RAM -> Encoder, 5 Bilder ins Nichts.
pub fn probe_sw_args(enc: Encoder) -> Vec<String> {
    let mut a = v(&["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30", "-frames:v", "5"]);
    a.extend(encoder_args(enc, false));
    a.extend(v(&["-f", "null", "-"]));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_caps() -> Caps {
        Caps {
            ddagrab_x264: true,
            hw_path: vec![Encoder::Nvenc, Encoder::Qsv],
            sw_path: vec![Encoder::Nvenc, Encoder::Qsv, Encoder::Amf],
        }
    }

    fn job(monitor: Option<Area>, area: Area, full: bool) -> Job {
        Job { fps: 30, area, full_desktop: full, monitor }
    }

    #[test]
    fn order_single_monitor() {
        let m = (0, 0, 1920, 1080);
        let p = plans(&job(Some(m), m, true), &all_caps(), true);
        let got: Vec<(Capture, Encoder)> = p.iter().map(|p| (p.capture, p.encoder)).collect();
        assert_eq!(
            got,
            vec![
                (Capture::Ddagrab, Encoder::Nvenc),
                (Capture::Ddagrab, Encoder::Qsv),
                (Capture::Ddagrab, Encoder::X264),
                (Capture::Gdigrab, Encoder::Nvenc),
                (Capture::Gdigrab, Encoder::Qsv),
                (Capture::Gdigrab, Encoder::Amf),
                (Capture::Gdigrab, Encoder::X264),
            ]
        );
    }

    #[test]
    fn multi_monitor_has_no_ddagrab() {
        let p = plans(&job(None, (0, 0, 3840, 1080), true), &all_caps(), true);
        assert!(p.iter().all(|p| p.capture == Capture::Gdigrab));
        assert_eq!(p.last().unwrap().encoder, Encoder::X264);
    }

    #[test]
    fn area_outside_monitor_has_no_ddagrab() {
        let m = (0, 0, 1920, 1080);
        let p = plans(&job(Some(m), (1900, 0, 100, 100), false), &all_caps(), true);
        assert!(p.iter().all(|p| p.capture == Capture::Gdigrab));
    }

    #[test]
    fn hw_disabled_is_plain_gdi_x264() {
        let m = (0, 0, 1920, 1080);
        let p = plans(&job(Some(m), m, true), &all_caps(), false);
        assert_eq!(p, vec![Plan { capture: Capture::Gdigrab, encoder: Encoder::X264 }]);
    }

    #[test]
    fn nothing_detected_still_has_fallback() {
        let m = (0, 0, 1920, 1080);
        let p = plans(&job(Some(m), m, true), &Caps::default(), true);
        assert_eq!(p, vec![Plan { capture: Capture::Gdigrab, encoder: Encoder::X264 }]);
    }

    #[test]
    fn ddagrab_full_monitor_has_no_crop() {
        let m = (0, 0, 1920, 1080);
        let a = video_args(Plan { capture: Capture::Ddagrab, encoder: Encoder::Nvenc }, &job(Some(m), m, true));
        let g = &a[1];
        assert_eq!(g, "ddagrab=output_idx=0:framerate=30:draw_mouse=1[v]");
        assert!(!a.contains(&"-pix_fmt".to_string()));
        assert!(a.contains(&"h264_nvenc".to_string()));
    }

    #[test]
    fn ddagrab_crop_is_relative_to_monitor() {
        let m = (100, 50, 1920, 1080);
        let a = video_args(
            Plan { capture: Capture::Ddagrab, encoder: Encoder::Qsv },
            &job(Some(m), (300, 250, 640, 360), false),
        );
        assert!(a[1].contains(":video_size=640x360:offset_x=200:offset_y=200"));
        assert!(a[1].ends_with(",hwmap=derive_device=qsv,format=qsv,vpp_qsv=format=nv12[v]"));
    }

    #[test]
    fn ddagrab_x264_downloads_and_converts() {
        let m = (0, 0, 1920, 1080);
        let a = video_args(Plan { capture: Capture::Ddagrab, encoder: Encoder::X264 }, &job(Some(m), m, true));
        assert!(a[1].contains(",hwdownload,format=bgra,pad="));
        let i = a.iter().position(|x| x == "-pix_fmt").unwrap();
        assert_eq!(a[i + 1], "yuv420p");
    }

    #[test]
    fn gdigrab_desktop_has_no_offsets_but_region_has() {
        let m = (0, 0, 1920, 1080);
        let p = Plan { capture: Capture::Gdigrab, encoder: Encoder::X264 };
        let full = video_args(p, &job(Some(m), m, true));
        assert!(!full.contains(&"-offset_x".to_string()));
        let reg = video_args(p, &job(Some(m), (10, 20, 640, 360), false));
        let i = reg.iter().position(|x| x == "-offset_x").unwrap();
        assert_eq!(&reg[i..i + 6], ["-offset_x", "10", "-offset_y", "20", "-video_size", "640x360"]);
    }

    #[test]
    fn gdigrab_qsv_uses_nv12() {
        let m = (0, 0, 1920, 1080);
        let a = video_args(Plan { capture: Capture::Gdigrab, encoder: Encoder::Qsv }, &job(Some(m), m, true));
        let i = a.iter().position(|x| x == "-pix_fmt").unwrap();
        assert_eq!(a[i + 1], "nv12");
    }

    #[test]
    fn summary_texts() {
        assert!(Caps::default().summary().starts_with("Keine"));
        assert!(all_caps().summary().contains("NVIDIA NVENC"));
        assert!(all_caps().summary().contains("GPU-Aufnahme"));
    }
}
