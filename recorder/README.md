# Bildschirm-Recorder

Portabler Bildschirm-Recorder mit Audio für Windows (Rust, egui, ffmpeg).

- Aufnahmebereich: gesamter Desktop, einzelner Monitor, einzelnes Fenster oder fester Bereich (per Maus wählbar)
- Audioquellen: beliebige Systemton-Geräte (Loopback) und Eingänge (Mikrofon), mehrere werden gemischt
- Speicherort: standardmäßig `videos\` neben der exe, frei wählbar
- Einstellungen werden in `settings.json` neben der exe gespeichert

`ffmpeg.exe` muss neben `recorder.exe` liegen (im Build-Artifact enthalten).
