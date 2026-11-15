use crate::config::exe_dir;
use crate::rec::ffcmd;
use anyhow::{anyhow, bail, Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

pub fn supports_ddagrab(p: &Path) -> bool {
    run_ok(p, &["-devices"]).map(|s| s.contains("ddagrab")).unwrap_or(false)
}

pub fn supports_qsv(p: &Path) -> bool {
    run_ok(p, &["-encoders"]).map(|s| s.contains("h264_qsv")).unwrap_or(false)
}

/// Prüft, ob die vorhandene ffmpeg.exe lauffähig ist und alles Nötige kann.
fn is_usable(p: &Path) -> bool {
    if !p.exists() {
        return false;
    }
    let has_x264 = run_ok(p, &["-encoders"]).map(|s| s.contains("libx264")).unwrap_or(false);
    let has_gdigrab = run_ok(p, &["-devices"]).map(|s| s.contains("gdigrab")).unwrap_or(false);
    let has_aac = run_ok(p, &["-encoders"]).map(|s| s.contains("aac")).unwrap_or(false);
    let has_ddagrab = supports_ddagrab(p);
    has_x264 && has_gdigrab && has_aac && (has_ddagrab || true)
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
            set(&state, &ctx, FfState::Ready);
            return;
        }
        let mut last_err = String::new();
        for url in URLS {
            match fetch(url, &state, &ctx) {
                Ok(()) => {
                    set(&state, &ctx, FfState::Ready);
                    return;
                }
                Err(e) => last_err = format!("{e:#}"),
            }
        }
        set(&state, &ctx, FfState::Failed(last_err));
    });
}
