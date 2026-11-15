//! Lädt ffmpeg beim ersten Start automatisch neben die exe (portabel) und prüft,
//! dass die Version die benötigten Funktionen hat (gdigrab, libx264, aac, amix).
//! Ermittelt außerdem per Probelauf, welche Hardwarebeschleunigung (ddagrab, NVENC, QSV, AMF)
//! auf diesem Rechner wirklich funktioniert.

use crate::config::exe_dir;
use crate::plan::{probe_ddagrab_args, probe_sw_args, Caps, Encoder};
use crate::rec::ffcmd;
use anyhow::{anyhow, bail, Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum FfState {
    Checking,
    Downloading { done: u64, total: u64 },
    Extracting,
    Ready,
    Failed(String),
}

/// Download-Quellen (GPL-Builds mit libx264), werden der Reihe nach probiert.
const URLS: [&str; 2] = [
    "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
];

pub fn path() -> PathBuf {
    exe_dir().join("ffmpeg.exe")
}

fn run_ok(p: &Path, args: &[&str]) -> Option<String> {
    let out = ffcmd(p).args(["-hide_banner"]).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

/// Führt ffmpeg mit den Argumenten aus und meldet, ob es erfolgreich war (max. 12 s).
fn probe(p: &Path, args: &[String]) -> bool {
    let mut c = ffcmd(p);
    c.args(["-y", "-hide_banner", "-loglevel", "error", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = c.spawn() else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) => {}
            Err(_) => return false,
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Ermittelt per Probelauf, was auf diesem Rechner wirklich funktioniert. Die Encoderliste von
/// ffmpeg reicht dafür nicht: sie zeigt nur, was einkompiliert ist (`h264_qsv` steht auch ohne
/// Intel-GPU drin). Die Probeläufe laufen parallel.
fn detect(p: &Path) -> Caps {
    thread::scope(|sc| {
        let dd = sc.spawn(|| probe(p, &probe_ddagrab_args(Encoder::X264)));
        let hw: Vec<_> = Encoder::HW
            .iter()
            .map(|&e| sc.spawn(move || (e, probe(p, &probe_ddagrab_args(e)), probe(p, &probe_sw_args(e)))))
            .collect();
        let mut caps = Caps { ddagrab_x264: dd.join().unwrap_or(false), ..Default::default() };
        for h in hw {
            if let Ok((e, gpu_frames, ram_frames)) = h.join() {
                if gpu_frames {
                    caps.hw_path.push(e);
                }
                if ram_frames {
                    caps.sw_path.push(e);
                }
            }
        }
        caps
    })
}

static CAPS: OnceLock<Caps> = OnceLock::new();

/// Fähigkeiten dieses Systems. Beim ersten Aufruf werden die Probeläufe gemacht (dauert
/// ein paar Sekunden), deshalb nicht im UI-Thread aufrufen - dort `caps_if_ready` benutzen.
pub fn caps() -> &'static Caps {
    CAPS.get_or_init(|| detect(&path()))
}

pub fn caps_if_ready() -> Option<&'static Caps> {
    CAPS.get()
}

/// Prüft, ob die vorhandene ffmpeg.exe lauffähig ist und alles Nötige kann.
fn is_usable(p: &Path) -> bool {
    if !p.exists() {
        return false;
    }

    let has_x264 = run_ok(p, &["-encoders"]).map(|s| s.contains("libx264")).unwrap_or(false);
    let has_gdigrab = run_ok(p, &["-devices"]).map(|s| s.contains("gdigrab")).unwrap_or(false);
    let has_aac = run_ok(p, &["-encoders"]).map(|s| s.contains("aac")).unwrap_or(false);

    has_x264 && has_gdigrab && has_aac
}

fn set(state: &Mutex<FfState>, ctx: &eframe::egui::Context, s: FfState) {
    *state.lock().unwrap() = s;
    ctx.request_repaint();
}

fn download(url: &str, dest: &Path, state: &Mutex<FfState>, ctx: &eframe::egui::Context) -> Result<()> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .build();
    let resp = agent.get(url).call().with_context(|| format!("Download von {url}"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut out = File::create(dest)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last = Instant::now();
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if last.elapsed() > Duration::from_millis(100) {
            set(state, ctx, FfState::Downloading { done, total });
            last = Instant::now();
        }
    }
    out.flush()?;
    if total > 0 && done < total {
        bail!("Download unvollständig");
    }
    Ok(())
}

fn extract(zip_path: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(File::open(zip_path)?)?;
    let idx = (0..z.len())
        .find(|&i| {
            z.by_index(i)
                .map(|e| e.name().replace('\\', "/").ends_with("/bin/ffmpeg.exe"))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("ffmpeg.exe im Archiv nicht gefunden"))?;
    let mut entry = z.by_index(idx)?;
    let part = exe_dir().join("ffmpeg.exe.part");
    {
        let mut out = File::create(&part).context("ffmpeg.exe konnte nicht neben der exe angelegt werden (Schreibrechte?)")?;
        std::io::copy(&mut entry, &mut out)?;
    }
    let dest = path();
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest)?;
    Ok(())
}

fn fetch(url: &str, state: &Mutex<FfState>, ctx: &eframe::egui::Context) -> Result<()> {
    let tmp = std::env::temp_dir().join("recorder_ffmpeg_download.zip");
    let res = (|| -> Result<()> {
        set(state, ctx, FfState::Downloading { done: 0, total: 0 });
        download(url, &tmp, state, ctx)?;
        set(state, ctx, FfState::Extracting);
        extract(&tmp)?;
        if !is_usable(&path()) {
            let _ = std::fs::remove_file(path());
            bail!("Heruntergeladene ffmpeg-Version ist nicht verwendbar");
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Prüft ffmpeg und lädt es bei Bedarf herunter (läuft im Hintergrund).
pub fn ensure(state: Arc<Mutex<FfState>>, ctx: eframe::egui::Context) {
    thread::spawn(move || {
        set(&state, &ctx, FfState::Checking);
        if is_usable(&path()) {
            let _ = caps(); // Hardware-Probeläufe, solange der Status noch "Checking" ist
            set(&state, &ctx, FfState::Ready);
            return;
        }
        let mut last_err = String::new();
        for url in URLS {
            match fetch(url, &state, &ctx) {
                Ok(()) => {
                    set(&state, &ctx, FfState::Checking);
                    let _ = caps();
                    set(&state, &ctx, FfState::Ready);
                    return;
                }
                Err(e) => last_err = format!("{e:#}"),
            }
        }
        set(&state, &ctx, FfState::Failed(last_err));
    });
}
