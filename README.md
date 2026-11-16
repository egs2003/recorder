# Bildschirm-Recorder

Portabler Bildschirm-Recorder mit Audio für Windows (Rust, egui, ffmpeg).

- Aufnahmebereich: gesamter Desktop, einzelner Monitor, einzelnes Fenster oder fester Bereich (per Maus wählbar)
- Audioquellen: beliebige Systemton-Geräte (Loopback) und Eingänge (Mikrofon), mehrere werden gemischt
- Speicherort: standardmäßig `videos\` neben der exe, frei wählbar
- Einstellungen werden in `settings.json` neben der exe gespeichert
- Monitor-Auswahl: "Aktualisieren" liest die Monitorliste neu ein (z. B. nach dem Anstecken eines Monitors). Der Button "Identifizieren" blendet auf jedem Monitor kurz dessen Nummer ein (der gewählte Monitor ist blau). Das passiert auch automatisch, wenn ein anderer Monitor gewählt wird.
- Design: dunkel oder hell, automatisch passend zur Windows-Einstellung (Wechsel zur Laufzeit wird übernommen)
- Hardwarebeschleunigung (abschaltbar unter "Optionen"): Beim ersten Start wird per Probelauf geprüft, was dieser Rechner wirklich kann, nicht nur, was in ffmpeg einkompiliert ist.
  Versucht wird der Reihe nach: GPU-Aufnahme (ddagrab) + NVENC / QSV / AMF, GPU-Aufnahme + x264, GDI-Aufnahme + NVENC / QSV / AMF, zuletzt GDI-Aufnahme + x264 (läuft immer).
  Schlägt ein Start fehl, wird automatisch die nächste Variante probiert. Das verwendete Verfahren steht während der Aufnahme in der Statuszeile.
  Die GPU-Aufnahme (ddagrab) erfasst nur einen Monitor und wird deshalb nur bei genau einem angeschlossenen Monitor verwendet.

`ffmpeg.exe` wird beim ersten Start automatisch neben die exe heruntergeladen (einmalig, ca. 100 MB, Internet nötig).
