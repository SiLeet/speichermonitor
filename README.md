# Speichermonitor

Windows-Programm gegen vollen Speicher – eine Exe, keine Installation.

- **Laufwerke:** alle lokalen Laufwerke mit Balken (belegt/frei/Größe); Klick startet die Analyse.
- **Was belegt Platz?** Schneller paralleler Scan (folgt keinen Junctions/Symlinks, Zugriffsfehler werden nur gezählt, abbrechbar). Ordnerbaum nach Größe mit Prozentbalken und Liste der 100 größten Dateien. Rechtsklick: „Im Explorer zeigen“, „In den Papierkorb …“ (immer mit Rückfrage, nie endgültig; Windows-Ordner, Programme-Ordner, ProgramData und Benutzerprofile selbst sind gesperrt).
- **Aufräumen:** bekannte Platzfresser mit gemessener Größe – Temp (Benutzer/Windows), Papierkorb, Windows-Update-Downloads, Lieferoptimierung, Browser-Zwischenspeicher (Edge/Chrome/Firefox, nur Cache), Absturzabbilder, Miniaturansichten, Komponentenspeicher (DISM), Ruhezustandsdatei (standardmäßig aus), Windows.old (eigene Rückfrage, über die Datenträgerbereinigung). Dateien in Gebrauch werden übersprungen und gezählt.
- Startet als Administrator, wenn der Nutzer Admin ist (Manifest `highestAvailable`); sonst sind Admin-Schritte ausgegraut, Knopf „Als Administrator neu starten“.

Bauen: `cargo build --release` · Tests: `$env:__COMPAT_LAYER='RunAsInvoker'; cargo test` · Starten: `target\release\Speichermonitor.exe`

Icon neu erzeugen: `powershell -NoProfile -File werkzeug\icon.ps1` (schreibt `speichermonitor.ico` und `icon64.rgba`).
