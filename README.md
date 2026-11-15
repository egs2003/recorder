# recorder

Einfacher Bildschirm-Recorder mit Audio für Windows.

- `recorder.exe` starten, Aufnahme läuft sofort
- Enter drücken zum Beenden
- Ergebnis: `Videos\aufnahme_JJJJMMTT_HHMMSS.mp4`

Optionen:
- `--mic`  Mikrofon statt Systemton aufnehmen
- `--fps N` Bildrate (Standard 30)

`ffmpeg.exe` muss neben `recorder.exe` liegen (ist im Build-Zip enthalten).
