//! Einfacher Bildschirm-Recorder mit Audio (Windows).
//!
//! - Video: ffmpeg (gdigrab, H.264)
//! - Audio: cpal/WASAPI, standardmäßig Systemton (Loopback des Standard-Ausgabegeräts),
//!   mit `--mic` stattdessen das Standard-Mikrofon.
//! - Start: Programm starten, Aufnahme läuft. Stop: Enter drücken.
//! - Ergebnis: <Videos>\aufnahme_JJJJMMTT_HHMMSS.mp4

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

type Wav = hound::WavWriter<BufWriter<File>>;

struct AudioState {
    writer: Option<Wav>,
    start: Instant,
    rate: u32,
    channels: u16,
    frames: u64,
}

impl AudioState {
    /// Schreibt Samples. WASAPI-Loopback liefert bei Stille keine Daten,
    /// daher werden Lücken anhand der Uhrzeit mit Stille aufgefüllt.
    fn push(&mut self, data: &[f32]) {
        let ch = self.channels.max(1) as u64;
        let incoming = data.len() as u64 / ch;
        let elapsed = (self.start.elapsed().as_secs_f64() * self.rate as f64) as u64;
        let expected_before = elapsed.saturating_sub(incoming);
        let tolerance = (self.rate / 10) as u64; // 100 ms
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

fn find_ffmpeg() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("ffmpeg.exe");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("ffmpeg")
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let use_mic = args.iter().any(|a| a == "--mic");
    let fps = args
        .iter()
        .position(|a| a == "--fps")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(30);

    let ffmpeg = find_ffmpeg();
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let out_dir = dirs::video_dir().unwrap_or_else(|| PathBuf::from("."));
    let out_file = out_dir.join(format!("aufnahme_{stamp}.mp4"));
    let tmp = std::env::temp_dir();
    let video_tmp = tmp.join(format!("rec_{stamp}_video.mp4"));
    let audio_tmp = tmp.join(format!("rec_{stamp}_audio.wav"));

    // --- Audio vorbereiten ---
    let host = cpal::default_host();
    let (device, cfg) = if use_mic {
        let d = host
            .default_input_device()
            .ok_or_else(|| anyhow!("Kein Standard-Mikrofon gefunden"))?;
        let c = d.default_input_config()?;
        (d, c)
    } else {
        let d = host
            .default_output_device()
            .ok_or_else(|| anyhow!("Kein Standard-Ausgabegerät gefunden"))?;
        let c = d.default_output_config()?;
        (d, c)
    };
    println!(
        "Audioquelle: {} ({})",
        device.name().unwrap_or_else(|_| "?".into()),
        if use_mic { "Mikrofon" } else { "Systemton" }
    );

    let spec = hound::WavSpec {
        channels: cfg.channels(),
        sample_rate: cfg.sample_rate().0,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let writer = hound::WavWriter::create(&audio_tmp, spec).context("WAV-Datei anlegen")?;
    let state = Arc::new(Mutex::new(AudioState {
        writer: Some(writer),
        start: Instant::now(),
        rate: cfg.sample_rate().0,
        channels: cfg.channels(),
        frames: 0,
    }));
    let stream = build_stream(&device, &cfg, state.clone())?;

    // --- Video starten ---
    let mut ff = Command::new(&ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-nostats"])
        .args(["-f", "gdigrab", "-framerate", &fps.to_string(), "-draw_mouse", "1", "-i", "desktop"])
        .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "23", "-pix_fmt", "yuv420p"])
        .args(["-progress", "pipe:1"])
        .arg(&video_tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("ffmpeg konnte nicht gestartet werden ({})", ffmpeg.display()))?;

    // Auf das erste aufgenommene Videobild warten, damit Audio und Video synchron starten.
    let stdout = ff.stdout.take().unwrap();
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
    if rx.recv_timeout(Duration::from_secs(15)).is_err() {
        let _ = ff.kill();
        bail!("ffmpeg liefert keine Videobilder.");
    }

    state.lock().unwrap().start = Instant::now();
    stream.play()?;

    println!("Aufnahme läuft ... Enter drücken zum Beenden.");
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;

    // --- Stoppen ---
    drop(stream);
    state.lock().unwrap().finish();
    if let Some(mut stdin) = ff.stdin.take() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    ff.wait()?;

    // --- Zusammenführen ---
    println!("Speichere ...");
    let status = Command::new(&ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(&video_tmp)
        .arg("-i")
        .arg(&audio_tmp)
        .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest"])
        .arg(&out_file)
        .status()?;
    if !status.success() {
        bail!("Zusammenführen fehlgeschlagen. Temporäre Dateien: {} / {}", video_tmp.display(), audio_tmp.display());
    }
    let _ = std::fs::remove_file(&video_tmp);
    let _ = std::fs::remove_file(&audio_tmp);
    println!("Fertig: {}", out_file.display());
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Fehler: {e:#}");
        eprintln!("Enter zum Schließen ...");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        std::process::exit(1);
    }
}
