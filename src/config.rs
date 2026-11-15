use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum Mode {
    #[default]
    Desktop,
    Monitor,
    Window,
    Region,
}

/// Audioquelle: `output = true` bedeutet Systemton (Loopback eines Ausgabegeräts),
/// sonst ein Eingabegerät (Mikrofon, Line-In ...).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct SourceId {
    pub output: bool,
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Default for Region {
    fn default() -> Self {
        Region { x: 0, y: 0, w: 1280, h: 720 }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    #[serde(skip)]
    pub mode: Mode,
    #[serde(skip)]
    pub monitor: usize,
    #[serde(skip)]
    pub window_title: String,
    #[serde(skip)]
    pub region: Region,
    /// Leer = Ordner "videos" neben der exe
    pub out_dir: String,
    pub fps: u32,
    #[serde(skip)]
    pub audio: Vec<SourceId>,
    #[serde(skip)]
    pub audio_initialized: bool,
    pub minimize: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: Mode::Desktop,
            monitor: 0,
            window_title: String::new(),
            region: Region::default(),
            out_dir: String::new(),
            fps: 30,
            audio: Vec::new(),
            audio_initialized: false,
            minimize: true,
        }
    }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_out_dir() -> PathBuf {
    exe_dir().join("videos")
}

fn settings_path() -> PathBuf {
    exe_dir().join("settings.json")
}

impl Settings {
    pub fn resolved_out_dir(&self) -> PathBuf {
        let t = self.out_dir.trim();
        if t.is_empty() {
            default_out_dir()
        } else {
            PathBuf::from(t)
        }
    }

    pub fn load() -> Self {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(settings_path(), s);
        }
    }
}
