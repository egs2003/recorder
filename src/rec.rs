use crate::config::{Mode, Settings, SourceId};
use crate::indicator::Indicator;
use crate::plan::{self, Area, Job, Plan};
use crate::winapi;
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum RecState {
    Starting,
    Recording(Instant),
    Saving,
    Done(PathBuf),
    Failed(String),
}

pub struct Handle {
    pub stop: Arc<AtomicBool>,
    pub state: Arc<Mutex<RecState>>,
    /// Beschreibung des tatsächlich verwendeten Verfahrens, z. B. "GPU-Aufnahme + NVIDIA NVENC"
    pub info: Arc<Mutex<String>>,
}

// ---------------------------------------------------------------- ffmpeg

fn find_ffmpeg() -> PathBuf {
    crate::ffmpeg::path()
}

pub fn ffcmd(p: &Path) -> Command {
    let mut c = Command::new(p);
    // ffmpeg.exe ist nicht DPI-aware; so bekommt gdigrab echte Pixel-Koordinaten
    c.env("__COMPAT_LAYER", "HighDpiAware");
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

fn log_tail(p: &Path) -> String {
    let s = std::fs::read_to_string(p).unwrap_or_default();
    let s = s.trim();
    let start = s.len().saturating_sub(600);
    let start = (start..=s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    s[start..].to_string()
}

/// Schneidet den Bereich auf den virtuellen Desktop zu und macht Breite/Höhe gerade.
fn clamp_area(a: Area, v: Area) -> Result<Area> {
    let x0 = a.0.max(v.0);
    let y0 = a.1.max(v.1);
    let x1 = (a.0 + a.2).min(v.0 + v.2);
    let y1 = (a.1 + a.3).min(v.1 + v.3);
    let (w, h) = ((x1 - x0) & !1, (y1 - y0) & !1);
    if w < 16 || h < 16 {
        bail!("Der Aufnahmebereich liegt außerhalb des Bildschirms oder ist zu klein");
    }
    Ok((x0, y0, w, h))
}

/// Ermittelt den aufzunehmenden Bereich (virtuelle Desktop-Koordinaten). Der zweite Wert sagt,
/// ob es der gesamte virtuelle Desktop ist.
fn capture_area(s: &Settings, mons: &[winapi::Monitor], virt: Area) -> Result<(Area, bool)> {
    match s.mode {
        Mode::Desktop => Ok((virt, true)),
        Mode::Monitor => {
            let m = mons
                .get(s.monitor)
                .ok_or_else(|| anyhow!("Monitor {} nicht gefunden", s.monitor + 1))?;
            Ok(((m.x, m.y, m.w, m.h), false))
        }
        Mode::Window => {
            if s.window_title.trim().is_empty() {
                bail!("Kein Fenster ausgewählt");
            }
            let hwnd = winapi::find_window(&s.window_title)
                .ok_or_else(|| anyhow!("Fenster nicht gefunden: {}", s.window_title))?;
            winapi::bring_to_front(hwnd);
            thread::sleep(Duration::from_millis(500));
            let r = winapi::window_rect(hwnd).ok_or_else(|| anyhow!("Fensterposition nicht ermittelbar"))?;
            Ok((clamp_area(r, virt)?, false))
        }
        Mode::Region => {
            let r = &s.region;
            Ok((clamp_area((r.x, r.y, r.w, r.h), virt)?, false))
        }
    }
}

/// Einzelbild des gesamten Desktops (für die Bereichsauswahl). Immer per gdigrab: nur das
/// erfasst zuverlässig alle Monitore zusammen.
pub fn screenshot() -> Result<(usize, usize, Vec<u8>)> {
    let ff = find_ffmpeg();
    let png = std::env::temp_dir().join("rec_screenshot.png");
    let _ = std::fs::remove_file(&png);
    let out = ffcmd(&ff)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "gdigrab", "-draw_mouse", "0", "-framerate", "1", "-i", "desktop"])
        .args(["-frames:v", "1", "-vf", "scale=1920:-1"])
        .arg(&png)
        .output()
        .context("ffmpeg.exe konnte nicht gestartet werden")?;
    if !out.status.success() {
        bail!("Screenshot fehlgeschlagen: {}", String::from_utf8_lossy(&out.stderr));
    }
    let img = image::open(&png)?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let _ = std::fs::remove_file(&png);
    Ok((w, h, img.into_raw()))
}

// ---------------------------------------------------------------- Audio

type Wav = hound::WavWriter<BufWriter<File>>;

struct AudioState {
    writer: Option<Wav>,
    start: Instant,
    rate: u32,
    channels: u16,
    frames: u64,
}

impl AudioState {
    fn push(&mut self, data: &[f32]) {
        let ch = self.channels.max(1) as u64;
        let incoming = data.len() as u64 / ch;
        let elapsed = (self.start.elapsed().as_secs_f64() * self.rate as f64) as u64;
        let expected_before = elapsed.saturating_sub(incoming);
        let tolerance = (self.rate / 10) as u64;
        if self.frames + tolerance < expected_before {
            let gap = expected_before - self.frames;
            self.write_silence(gap);
        }
        if let Some(w) = self.writer.as_mut() {
            for &s in data {
                let _ = w.write_sample(s);
            }
        }
        self.frames += incoming;
    }

    fn write_silence(&mut self, frames: u64) {
        let ch = self.channels.max(1) as u64;
        if let Some(w) = self.writer.as_mut() {
            for _ in 0..frames * ch {
                let _ = w.write_sample(0.0f32);
            }
        }
        self.frames += frames;
    }

    fn finish(&mut self) {
        let elapsed = (self.start.elapsed().as_secs_f64() * self.rate as f64) as u64;
        if self.frames < elapsed {
            self.write_silence(elapsed - self.frames);
        }
        if let Some(w) = self.writer.take() {
            let _ = w.finalize();
        }
    }
}

fn build_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    state: Arc<Mutex<AudioState>>,
) -> Result<Stream> {
    let config: cpal::StreamConfig = cfg.clone().into();
    let err_fn = |e| eprintln!("Audio-Fehler: {e}");
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |d: &[f32], _| state.lock().unwrap().push(d),
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |d: &[i16], _| {
                let v: Vec<f32> = d.iter().map(|&s| s as f32 / 32768.0).collect();
                state.lock().unwrap().push(&v);
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            &config,
            move |d: &[u16], _| {
                let v: Vec<f32> = d.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                state.lock().unwrap().push(&v);
            },
            err_fn,
            None,
        )?,
        f => bail!("Nicht unterstütztes Audioformat: {f:?}"),
    };
    Ok(stream)
}

fn open_audio(host: &cpal::Host, id: &SourceId, wav: &Path) -> Result<(Stream, Arc<Mutex<AudioState>>)> {
    let (device, cfg) = if id.output {
        let d = host
            .output_devices()?
            .find(|d| d.name().map(|n| n == id.name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Audiogerät nicht gefunden: {}", id.name))?;
        let c = d.default_output_config()?;
        (d, c)
    } else {
        let d = host
            .input_devices()?
            .find(|d| d.name().map(|n| n == id.name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Audiogerät nicht gefunden: {}", id.name))?;
        let c = d.default_input_config()?;
        (d, c)
    };
    let spec = hound::WavSpec {
        channels: cfg.channels(),
        sample_rate: cfg.sample_rate().0,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let writer = hound::WavWriter::create(wav, spec).context("WAV-Datei anlegen")?;
    let state = Arc::new(Mutex::new(AudioState {
        writer: Some(writer),
        start: Instant::now(),
        rate: cfg.sample_rate().0,
        channels: cfg.channels(),
        frames: 0,
    }));
    let stream = build_stream(&device, &cfg, state.clone())?;
    Ok((stream, state))
}

// ---------------------------------------------------------------- Aufnahme

pub fn start(s: Settings, ctx: eframe::egui::Context) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(RecState::Starting));
    let info = Arc::new(Mutex::new(String::new()));
    let (stop2, state2, info2) = (stop.clone(), state.clone(), info.clone());
    thread::spawn(move || {
        let res = run(&s, &stop2, &state2, &info2, &ctx);
        *state2.lock().unwrap() = match res {
            Ok(p) => RecState::Done(p),
            Err(e) => RecState::Failed(format!("{e:#}")),
        };
        if s.minimize {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    });
    Handle { stop, state, info }
}

/// Startet ffmpeg mit einem Aufnahmeplan und wartet auf das erste aufgenommene Bild.
/// Schlägt der Start fehl (Encoder/GPU nicht verfügbar, Zeitüberschreitung), wird ffmpeg
/// beendet und ein Fehler mit dem Ende des ffmpeg-Logs zurückgegeben.
fn start_ffmpeg(
    ffmpeg: &Path,
    pl: Plan,
    job: &Job,
    video_tmp: &Path,
    log_path: &Path,
    stop: &AtomicBool,
) -> Result<Child> {
    let log = File::create(log_path)?;
    let mut cmd = ffcmd(ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-nostats"])
        .args(plan::video_args(pl, job))
        .args(["-progress", "pipe:1"])
        .arg(video_tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    let mut ff = cmd
        .spawn()
        .context("ffmpeg.exe konnte nicht gestartet werden (muss neben recorder.exe liegen)")?;

    let stdout = ff.stdout.take().context("ffmpeg-Ausgabe nicht verfügbar")?;
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !sent {
                if let Some(n) = line.strip_prefix("frame=") {
                    if n.trim().parse::<u64>().unwrap_or(0) > 0 {
                        let _ = tx.send(());
                        sent = true;
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => return Ok(ff),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = ff.kill();
                let _ = ff.wait();
                bail!("ffmpeg lieferte keine Bilder: {}", log_tail(log_path));
            }
        }
        if stop.load(Ordering::SeqCst) {
            let _ = ff.kill();
            let _ = ff.wait();
            bail!("Abgebrochen");
        }
        if Instant::now() > deadline {
            let _ = ff.kill();
            let _ = ff.wait();
            bail!("Zeitüberschreitung beim Start: {}", log_tail(log_path));
        }
    }
}

fn run(
    s: &Settings,
    stop: &AtomicBool,
    state: &Mutex<RecState>,
    info: &Mutex<String>,
    ctx: &eframe::egui::Context,
) -> Result<PathBuf> {
    let ffmpeg = find_ffmpeg();
    let out_dir = s.resolved_out_dir();
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("Ausgabeordner nicht erstellbar: {}", out_dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let out_file = out_dir.join(format!("aufnahme_{stamp}.mp4"));
    let tmp = std::env::temp_dir();
    let video_tmp = tmp.join(format!("rec_{stamp}_video.mp4"));
    let log_path = tmp.join(format!("rec_{stamp}_ffmpeg.log"));

    let host = cpal::default_host();
    let mut audio: Vec<(Stream, Arc<Mutex<AudioState>>, PathBuf)> = Vec::new();
    for (i, id) in s.audio.iter().enumerate() {
        let wav = tmp.join(format!("rec_{stamp}_a{i}.wav"));
        let (stream, st) = open_audio(&host, id, &wav)?;
        audio.push((stream, st, wav));
    }

    if s.minimize {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(true));
        thread::sleep(Duration::from_millis(700));
    }

    let mons = winapi::list_monitors();
    let virt = winapi::virtual_bounds(&mons);
    let (area, full_desktop) = capture_area(s, &mons, virt)?;
    let monitor = match mons.as_slice() {
        [m] => Some((m.x, m.y, m.w, m.h)),
        _ => None,
    };
    let job = Job { fps: s.fps, area, full_desktop, monitor };
    // Hardware-Probeläufe sind normalerweise schon beim Programmstart passiert
    let plans = plan::plans(&job, crate::ffmpeg::caps(), s.hw_accel);
    let indicator = Indicator::show(area.0, area.1, area.2, area.3);

    // Pläne der Reihe nach probieren, bis einer Bilder liefert
    let mut started: Option<(Child, Plan)> = None;
    let mut tried: Vec<String> = Vec::new();
    let mut last_err = String::new();
    for p in &plans {
        match start_ffmpeg(&ffmpeg, *p, &job, &video_tmp, &log_path, stop) {
            Ok(child) => {
                started = Some((child, *p));
                break;
            }
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    bail!("Abgebrochen");
                }
                tried.push(p.label());
                last_err = format!("{e:#}");
            }
        }
    }
    let Some((mut ff, used)) = started else {
        bail!(
            "Aufnahme konnte nicht gestartet werden (versucht: {}). Letzter Fehler: {}",
            tried.join(", "),
            last_err
        );
    };
    *info.lock().unwrap() = used.label();

    let now = Instant::now();
    for (stream, st, _) in &audio {
        st.lock().unwrap().start = now;
        stream.play()?;
    }
    *state.lock().unwrap() = RecState::Recording(now);

    while !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
        if let Ok(Some(_)) = ff.try_wait() {
            bail!("ffmpeg wurde unerwartet beendet: {}", log_tail(&log_path));
        }
    }

    *state.lock().unwrap() = RecState::Saving;
    drop(indicator);
    let mut audio_files = Vec::new();
    for (stream, st, wav) in audio {
        drop(stream);
        st.lock().unwrap().finish();
        audio_files.push(wav);
    }
    if let Some(mut stdin) = ff.stdin.take() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    ff.wait()?;

    let mut cmd = ffcmd(&ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(&video_tmp);
    for a in &audio_files {
        cmd.arg("-i").arg(a);
    }
    match audio_files.len() {
        0 => {
            cmd.args(["-c:v", "copy"]);
        }
        1 => {
            cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
        }
        n => {
            let inputs: String = (1..=n).map(|i| format!("[{i}:a]")).collect();
            let filter = format!("{inputs}amix=inputs={n}:normalize=0:duration=longest[a]");
            cmd.args(["-filter_complex", filter.as_str(), "-map", "0:v", "-map", "[a]"]);
            cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
        }
    }
    cmd.args(["-movflags", "+faststart"]).arg(&out_file);
    let out = cmd.output().context("ffmpeg (Zusammenführen)")?;
    if !out.status.success() {
        bail!(
            "Zusammenführen fehlgeschlagen: {}\n(Temporäre Dateien in {})",
            String::from_utf8_lossy(&out.stderr),
            tmp.display()
        );
    }
    let _ = std::fs::remove_file(&video_tmp);
    let _ = std::fs::remove_file(&log_path);
    for a in &audio_files {
        let _ = std::fs::remove_file(a);
    }
    Ok(out_file)
}
