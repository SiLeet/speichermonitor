//! Sperrliste, Pfadprüfung und sicheres Löschen (nie Verknüpfungen folgen).

use crate::scan::{ATTR_ORDNER, ATTR_REPARSE};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::SystemTime;

/// Kleinschreibung, `\` statt `/`; nur absolute Laufwerkspfade ohne `..`.
pub fn norm(p: &Path) -> Option<PathBuf> {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    let p = PathBuf::from(s.trim());
    let mut c = p.components();
    let laufwerk =
        matches!(c.next(), Some(Component::Prefix(px)) if matches!(px.kind(), Prefix::Disk(_)));
    let wurzel = matches!(c.next(), Some(Component::RootDir));
    let sauber = c.all(|t| matches!(t, Component::Normal(_)));
    (laufwerk && wurzel && sauber).then_some(p)
}

/// Absoluter Pfad aus einer Umgebungsvariable; leer, Wurzel oder Laufwerkswurzel = None.
pub fn basis(wert: Option<String>) -> Option<PathBuf> {
    let w = wert?;
    let n = norm(Path::new(&w))?;
    (n.components().count() > 2).then(|| PathBuf::from(w.trim()))
}

pub struct Schutz {
    windows: PathBuf,
    benutzer: PathBuf,
    geschuetzt: Vec<PathBuf>,
}

impl Schutz {
    pub fn aus_umgebung(env: &dyn Fn(&str) -> Option<String>) -> Schutz {
        let lw = env("SystemDrive")
            .filter(|s| matches!(s.as_bytes(), [b, b':'] if b.is_ascii_alphabetic()))
            .unwrap_or_else(|| "C:".into())
            .to_lowercase();
        let n = |s: String| norm(Path::new(&s));
        // lw ist geprüft, also sind diese Pfade schon in norm-Form.
        let windows = env("SystemRoot")
            .and_then(n)
            .unwrap_or_else(|| PathBuf::from(format!("{lw}\\windows")));
        let benutzer = PathBuf::from(format!("{lw}\\users"));
        let mut g: Vec<PathBuf> = ["Program Files", "Program Files (x86)", "ProgramData"]
            .iter()
            .filter_map(|d| n(format!("{lw}\\{d}")))
            .collect();
        for k in [
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            g.extend(env(k).and_then(n));
        }
        if let Some(profil) = env("USERPROFILE").and_then(n) {
            for unter in [
                "",
                "appdata",
                "appdata\\local",
                "appdata\\roaming",
                "desktop",
                "documents",
                "downloads",
            ] {
                g.push(profil.join(unter));
            }
        }
        g.push(windows.join("system32"));
        g.push(windows.clone());
        g.push(benutzer.clone());
        Schutz {
            windows,
            benutzer,
            geschuetzt: g,
        }
    }

    pub fn system() -> Schutz {
        Self::aus_umgebung(&|k| std::env::var(k).ok())
    }

    fn pruefe(&self, p: &Path) -> Result<PathBuf, &'static str> {
        let n = norm(p).ok_or("Ungültiger Pfad")?;
        if n.components().count() <= 2 {
            return Err("Ein ganzes Laufwerk wird nie gelöscht.");
        }
        if n.parent() == Some(self.benutzer.as_path()) {
            return Err("Benutzerprofile werden nie gelöscht.");
        }
        if self.geschuetzt.iter().any(|g| g.starts_with(&n)) {
            return Err("Dieser Ordner wird von Windows gebraucht und ist gesperrt.");
        }
        Ok(n)
    }

    /// Darf der Inhalt dieses Ordners beim Aufräumen gelöscht werden?
    pub fn pruefe_aufraeumordner(&self, p: &Path) -> Result<(), &'static str> {
        self.pruefe(p).map(|_| ())
    }

    /// Darf dieser Ordner bzw. diese Datei aus der Analyse in den Papierkorb?
    pub fn pruefe_papierkorb(&self, p: &Path) -> Result<(), &'static str> {
        if self.pruefe(p)?.starts_with(&self.windows) {
            return Err("Dateien von Windows werden nie gelöscht.");
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Bilanz {
    pub frei: u64,
    pub geloescht: u64,
    pub uebersprungen: u64,
    /// Ein Windows-Befehl (DISM, powercfg, …) ist fehlgeschlagen.
    pub fehler: bool,
}

impl Bilanz {
    pub fn plus(&mut self, b: Bilanz) {
        self.frei += b.frei;
        self.geloescht += b.geloescht;
        self.uebersprungen += b.uebersprungen;
        self.fehler |= b.fehler;
    }
}

/// Löscht den INHALT von `dir` endgültig. Junctions/Symlinks darin werden nur als
/// Verweis entfernt, nie verfolgt. Ist `dir` selbst eine Verknüpfung: nichts tun.
pub fn leere_ordner(dir: &Path, b: &mut Bilanz) {
    leere_ordner_vor(dir, None, b);
}

/// Mit `vor`: nur Dateien, die seit `vor` unverändert sind - jüngere bleiben, ohne zu zählen.
pub fn leere_ordner_vor(dir: &Path, vor: Option<SystemTime>, b: &mut Bilanz) {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_attributes() & ATTR_REPARSE == 0 && m.is_dir() => leere(dir, vor, b),
        _ => {}
    }
}

/// Größe der Dateien, die seit `vor` unverändert sind (folgt keinen Verknüpfungen).
pub fn alte_groesse(dir: &Path, vor: SystemTime) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter_map(|e| Some((e.path(), e.metadata().ok()?)))
        .filter(|(_, m)| m.file_attributes() & ATTR_REPARSE == 0)
        .map(|(p, m)| {
            if m.is_dir() {
                alte_groesse(&p, vor)
            } else if m.modified().is_ok_and(|t| t <= vor) {
                m.len()
            } else {
                0
            }
        })
        .sum()
}

fn leere(dir: &Path, vor: Option<SystemTime>, b: &mut Bilanz) {
    let Ok(rd) = fs::read_dir(dir) else {
        b.uebersprungen += 1;
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(m) = e.metadata() else {
            b.uebersprungen += 1;
            continue;
        };
        let attr = m.file_attributes();
        if attr & ATTR_ORDNER == 0 && vor.is_some_and(|v| m.modified().map_or(true, |t| t > v)) {
            continue;
        }
        if attr & ATTR_REPARSE != 0 {
            let r = if attr & ATTR_ORDNER != 0 {
                fs::remove_dir(&p)
            } else {
                fs::remove_file(&p)
            };
            if r.is_err() {
                b.uebersprungen += 1;
            }
        } else if attr & ATTR_ORDNER != 0 {
            leere(&p, vor, b);
            let _ = fs::remove_dir(&p); // bleibt stehen, wenn noch Dateien in Gebrauch sind
        } else {
            loesche_datei(&p, m, b);
        }
    }
}

/// Löscht eine einzelne Datei (keine Verknüpfung, kein Ordner).
pub fn loesche_datei_pfad(p: &Path, b: &mut Bilanz) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.file_attributes() & (ATTR_REPARSE | ATTR_ORDNER) == 0 => loesche_datei(p, m, b),
        Ok(_) => b.uebersprungen += 1,
        Err(_) => {}
    }
}

fn loesche_datei(p: &Path, m: fs::Metadata, b: &mut Bilanz) {
    let mut r = fs::remove_file(p);
    if r.is_err() && m.permissions().readonly() {
        let mut perm = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // nur Windows: entfernt das Schreibschutz-Attribut
        perm.set_readonly(false);
        if fs::set_permissions(p, perm).is_ok() {
            r = fs::remove_file(p);
        }
    }
    match r {
        Ok(()) => {
            b.frei += m.len();
            b.geloescht += 1;
        }
        Err(_) => b.uebersprungen += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::tests::{junction, testordner};

    fn schutz() -> Schutz {
        Schutz::aus_umgebung(&|k| {
            match k {
                "SystemDrive" => Some("C:"),
                "SystemRoot" => Some("C:\\WINDOWS"),
                "USERPROFILE" => Some("C:\\Users\\Max"),
                "LOCALAPPDATA" => Some("C:\\Users\\Max\\AppData\\Local"),
                _ => None,
            }
            .map(String::from)
        })
    }

    #[test]
    fn pfadpruefung() {
        assert_eq!(basis(None), None);
        assert_eq!(basis(Some("".into())), None);
        assert_eq!(basis(Some("   ".into())), None);
        assert_eq!(basis(Some("C:\\".into())), None);
        assert_eq!(basis(Some("C:".into())), None);
        assert_eq!(basis(Some("\\".into())), None);
        assert_eq!(basis(Some("temp".into())), None);
        assert_eq!(basis(Some("C:\\a\\..\\..".into())), None);
        assert_eq!(basis(Some("\\\\server\\share\\x".into())), None);
        assert_eq!(
            basis(Some("D:\\Temp".into())),
            Some(PathBuf::from("D:\\Temp"))
        );
    }

    #[test]
    fn sperrliste() {
        let s = schutz();
        for p in [
            "C:\\",
            "c:\\windows",
            "C:/Windows/",
            "C:\\Windows\\System32",
            "C:\\Program Files",
            "C:\\program files (x86)",
            "C:\\ProgramData",
            "C:\\Users",
            "C:\\Users\\Max",
            "C:\\Users\\Anna",
            "C:\\Users\\Public",
            "C:\\Users\\Max\\AppData",
            "C:\\Users\\Max\\AppData\\Local",
            "C:\\Users\\Max\\Documents",
            "C:\\Users\\Max\\..\\Anna\\x",
        ] {
            assert!(
                s.pruefe_aufraeumordner(Path::new(p)).is_err(),
                "{p} muss gesperrt sein"
            );
        }
        for p in [
            "C:\\Windows\\Temp",
            "C:\\Users\\Max\\AppData\\Local\\Temp",
            "D:\\Temp",
        ] {
            assert!(
                s.pruefe_aufraeumordner(Path::new(p)).is_ok(),
                "{p} muss erlaubt sein"
            );
        }
        // Papierkorb: zusätzlich alles in Windows gesperrt
        assert!(s.pruefe_papierkorb(Path::new("C:\\Windows\\Temp")).is_err());
        assert!(s
            .pruefe_papierkorb(Path::new("C:\\Windows\\WinSxS"))
            .is_err());
        assert!(s.pruefe_papierkorb(Path::new("D:\\")).is_err());
        assert!(s
            .pruefe_papierkorb(Path::new("C:\\Users\\Max\\Videos\\urlaub.mp4"))
            .is_ok());
        assert!(s
            .pruefe_papierkorb(Path::new("C:\\Program Files\\AltesProgramm"))
            .is_ok());
    }

    #[test]
    fn kaputtes_systemlaufwerk_stuerzt_nicht_ab() {
        for lw in ["1:", "::", "", "CC", "ä:"] {
            let s = Schutz::aus_umgebung(&|k| (k == "SystemDrive").then(|| lw.to_string()));
            assert!(
                s.pruefe_aufraeumordner(Path::new("C:\\Windows")).is_err(),
                "SystemDrive={lw:?}: Rückfall auf C: fehlt"
            );
        }
    }

    #[test]
    fn leeren_folgt_keiner_junction() {
        let t = testordner("leeren");
        let ziel = t.join("ziel");
        let aussen = t.join("aussen");
        fs::create_dir_all(ziel.join("sub/tiefer")).unwrap();
        fs::create_dir_all(&aussen).unwrap();
        fs::write(ziel.join("a.tmp"), vec![1u8; 100]).unwrap();
        fs::write(ziel.join("sub/tiefer/b.tmp"), vec![1u8; 20]).unwrap();
        fs::write(ziel.join("nurlesen.tmp"), vec![1u8; 3]).unwrap();
        let mut perm = fs::metadata(ziel.join("nurlesen.tmp"))
            .unwrap()
            .permissions();
        perm.set_readonly(true);
        fs::set_permissions(ziel.join("nurlesen.tmp"), perm).unwrap();
        fs::write(aussen.join("wichtig.txt"), b"bleibt").unwrap();
        junction(&ziel.join("link"), &aussen);
        junction(&t.join("ziel-als-link"), &aussen);

        // In Gebrauch: offen ohne Teilen-Löschen-Recht
        use std::os::windows::fs::OpenOptionsExt;
        fs::write(ziel.join("offen.tmp"), vec![1u8; 7]).unwrap();
        let offen = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(ziel.join("offen.tmp"))
            .unwrap();

        let mut b = Bilanz::default();
        leere_ordner(&ziel, &mut b);
        assert_eq!(b.frei, 123);
        assert_eq!(b.geloescht, 3);
        assert_eq!(b.uebersprungen, 1);
        assert!(
            aussen.join("wichtig.txt").exists(),
            "Ziel der Junction wurde angefasst!"
        );
        assert!(!ziel.join("link").exists());
        assert!(!ziel.join("sub").exists());
        assert!(ziel.exists(), "der Ordner selbst bleibt");
        drop(offen);

        // Ein Aufräumziel, das selbst eine Junction ist, wird nicht angefasst.
        let mut b = Bilanz::default();
        leere_ordner(&t.join("ziel-als-link"), &mut b);
        assert_eq!(b.geloescht, 0);
        assert!(aussen.join("wichtig.txt").exists());

        // Einzeldatei über eine Junction: übersprungen
        let mut b = Bilanz::default();
        loesche_datei_pfad(&t.join("ziel-als-link"), &mut b);
        assert_eq!((b.geloescht, b.uebersprungen), (0, 1));
        fs::remove_dir_all(&t).unwrap();
    }
}
