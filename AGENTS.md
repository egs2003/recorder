# Bildschirm-Recorder: Hinweise für Agenten

Portabler Bildschirm-Recorder mit Audio für Windows. Rust, egui/eframe 0.29, ffmpeg als externe Programmdatei.
Eine Übersicht der Funktionen steht in `README.md`.

## Workflow

- Änderungen gehen **direkt auf `main`**, ohne Branch und Pull Request.
- **Bei jeder Änderung die Version in `Cargo.toml` erhöhen** (kleine Korrekturen: Patch, neue Funktionen: Minor).
  Der Build veröffentlicht pro Push ein Release `v<Version>` mit `recorder.exe`, und die Update-Funktion der App
  vergleicht dagegen. Ohne höhere Version erkennt kein Benutzer ein Update.
- Neue Funktionen oder geändertes Verhalten kurz in `README.md` nachtragen.
- Commit-Nachrichten auf Englisch, Oberflächentexte und Kommentare zur Bedienung auf Deutsch.

## Bauen und prüfen

- Das Programm ist **nur für Windows** (`windows-sys`, GDI, DXGI). Auf Linux lässt es sich nicht kompilieren.
  Geprüft wird über GitHub Actions (`.github/workflows/build.yml`). Fehlermeldungen kommen vom Benutzer aus dem Build-Log.
- Ohne Compiler: Signaturen der APIs im Quelltext der Bibliothek nachsehen (egui 0.29.1, windows-sys 0.52),
  nicht aus dem Gedächtnis schreiben. Reine Logik ohne Windows-Bezug (z. B. `plan.rs`, Versionsvergleich in `update.rs`)
  lässt sich getrennt mit `rustc --test` ausprobieren.
- Eine Oberfläche kann hier nicht angesehen werden. Optische Änderungen deshalb klein halten und das im Ergebnis sagen.

## Aufbau (`src/`)

| Datei | Inhalt |
|---|---|
| `main.rs` | Oberfläche (egui), Farbpalette, Theme, Zustand der App |
| `rec.rs` | Aufnahme: ffmpeg starten, Audio (cpal) in WAV, am Ende zusammenführen |
| `plan.rs` | ffmpeg-Argumente je Aufnahmeverfahren und Encoder, Fallback-Reihenfolge, mit Tests |
| `ffmpeg.rs` | ffmpeg herunterladen, Hardware per Probelauf erkennen (`caps()`) |
| `update.rs` | Selbst-Update über GitHub-Releases |
| `identify.rs` | Nummern-Einblendung zur Monitor-Identifizierung |
| `indicator.rs` | Rahmen um den Aufnahmebereich |
| `winapi.rs` | Monitore, Fenster, DPI |
| `config.rs` | Einstellungen (`settings.json` neben der exe) |

## Regeln und Fallen

**Hardwarebeschleunigung** (`plan.rs`, `rec.rs`)
- Ob ein Encoder läuft, wird per **Probelauf** ermittelt, nicht aus der Encoderliste von ffmpeg. `h264_qsv` steht dort
  auch ohne Intel-GPU drin.
- `ddagrab` ist ein ffmpeg-**Filter** (`-filter_complex`), kein Eingabegerät (`-f`).
- ddagrab erfasst nur einen Monitor und wird nur bei **genau einem** angeschlossenen Monitor verwendet.
- Es gibt immer einen letzten Ausweg (GDI + x264). Ein Fehlstart muss zum nächsten Verfahren führen, nie zum Abbruch.
  Neue Verfahren in `plans()` einordnen und dort mit einem Test absichern.

**Oberfläche** (`main.rs`)
- Farben nur über `pal()`, **keine festen Farbkonstanten** im Zeichencode. Es gibt ein dunkles und ein helles Design,
  das der Systemeinstellung folgt (`DARK`, `LIGHT`).
- `RichText::strong()` nimmt seine Farbe von `widgets.active.fg_stroke`. Diese Schriftfarbe nicht überschreiben.
- `selection.stroke` ist die **Schriftfarbe** ausgewählter Einträge (z. B. im Dropdown), `selection.bg_fill` deren
  Hintergrund. Beide dürfen nie dieselbe Farbe haben (blau auf blau war schon einmal unlesbar).
- Zeilen mit mehreren Widgets nicht in `right_to_left` mit abgeschätzter Breite bauen. Wird die Zeile breiter als die
  Karte, wächst das Fenster und der Inhalt rutscht nach links. Lieber auf mehrere Zeilen verteilen.
- Float-Literale in `Stroke::new` als `1.0_f32` schreiben, sonst gibt es Compiler-Warnungen.

**Update** (`update.rs`)
- Die Prüfung beim Start ist still, gefragt wird erst vor dem Herunterladen. Während einer Aufnahme ist das Update gesperrt.
- Der Pfad der exe wird beim Start gemerkt (`exe()`), weil `current_exe()` nach dem Umbenennen nicht verlässlich ist.
