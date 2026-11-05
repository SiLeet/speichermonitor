//! Bekannte Platzfresser: Auflösung der Pfade, Größe messen, aufräumen.

use crate::scan::ordner_groesse;
use crate::schutz::{basis, leere_ordner, loesche_datei_pfad, Bilanz, Schutz};
use crate::win;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Teil {
    /// Inhalt des Ordners löschen, der Ordner bleibt.
    Inhalt(PathBuf),
    Datei(PathBuf),
    /// Dateien `<präfix>*.db` direkt im Ordner.
    Muster(PathBuf, &'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Art {
    Dateien(Vec<Teil>),
    Papierkorb,
    WindowsUpdate(PathBuf),
    Lieferoptimierung(PathBuf),
    Komponentenspeicher,
    Ruhezustand(PathBuf),
    WindowsAlt(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Ziel {
    pub titel: &'static str,
    pub erklaerung: &'static str,
    pub art: Art,
    pub admin: bool,
    pub vorgewaehlt: bool,
    /// Exe-Name des Browsers: läuft er, wird das Ziel übersprungen.
    pub browser: Option<&'static str>,
}

fn ziel(titel: &'static str, erklaerung: &'static str, art: Art, admin: bool) -> Ziel {
    Ziel {
        titel,
        erklaerung,
        art,
        admin,
        vorgewaehlt: true,
        browser: None,
    }
}

fn unterordner(p: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(p) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

/// Systemlaufwerk als "C:\" (nur gültige Laufwerksbuchstaben).
fn systemlaufwerk(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let d = env("SystemDrive")?;
    let b = d.as_bytes();
    (b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        .then(|| PathBuf::from(format!("{d}\\")))
}

pub fn ziele(env: &dyn Fn(&str) -> Option<String>, schutz: &Schutz) -> Vec<Ziel> {
    // Jeder Ordner, dessen Inhalt gelöscht wird, muss die Sperrliste passieren.
    let ok = |p: PathBuf| schutz.pruefe_aufraeumordner(&p).is_ok().then_some(p);
    let inhalt = |p: PathBuf| ok(p).map(Teil::Inhalt);
    let win = basis(env("SystemRoot"));
    let local = basis(env("LOCALAPPDATA"));
    let lw = systemlaufwerk(env);
    let mut v = Vec::new();

    // Nur ein echter Temp-Ordner – zeigt TEMP auf einen Datenordner, kein Ziel.
    let ist_temp = |p: &PathBuf| {
        p.file_name()
            .is_some_and(|n| ["temp", "tmp"].contains(&n.to_string_lossy().to_lowercase().as_str()))
    };
    if let Some(t) = basis(env("TEMP")).filter(ist_temp).and_then(inhalt) {
        v.push(ziel(
            "Temporäre Dateien (Ihr Benutzer)",
            "Reste, die Programme beim Arbeiten anlegen und danach nicht mehr brauchen.",
            Art::Dateien(vec![t]),
            false,
        ));
    }
    if let Some(t) = win.clone().and_then(|w| inhalt(w.join("Temp"))) {
        v.push(ziel(
            "Temporäre Dateien (Windows)",
            "Reste von Windows und von Installationen, die nicht mehr gebraucht werden.",
            Art::Dateien(vec![t]),
            true,
        ));
    }
    v.push(Ziel {
        vorgewaehlt: false,
        ..ziel(
            "Papierkorb leeren",
            "Alles im Papierkorb wird endgültig gelöscht – vorher kurz hineinschauen.",
            Art::Papierkorb,
            false,
        )
    });
    if let Some(w) = &win {
        if let Some(p) = ok(w.join("SoftwareDistribution").join("Download")) {
            v.push(ziel(
                "Windows-Update-Downloads",
                "Bereits installierte Update-Pakete; Windows lädt sie bei Bedarf neu.",
                Art::WindowsUpdate(p),
                true,
            ));
        }
        let lo = w.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache");
        if let Some(p) = ok(lo) {
            v.push(ziel("Lieferoptimierung", "Update-Dateien, die Windows mit anderen PCs teilt; sie werden nicht mehr gebraucht.", Art::Lieferoptimierung(p), true));
        }
    }
    if let Some(l) = &local {
        for (titel, exe, ud) in [
            (
                "Edge-Zwischenspeicher",
                "msedge.exe",
                r"Microsoft\Edge\User Data",
            ),
            (
                "Chrome-Zwischenspeicher",
                "chrome.exe",
                r"Google\Chrome\User Data",
            ),
        ] {
            let teile: Vec<Teil> = unterordner(&l.join(ud))
                .iter()
                .flat_map(|profil| ["Cache", "Code Cache", "GPUCache"].map(|c| profil.join(c)))
                .filter(|p| p.is_dir())
                .filter_map(inhalt)
                .collect();
            if !teile.is_empty() {
                v.push(Ziel { browser: Some(exe), ..ziel(titel, "Zwischengespeicherte Teile von Webseiten; Lesezeichen, Passwörter und Verlauf bleiben erhalten.", Art::Dateien(teile), false) });
            }
        }
        let ff: Vec<Teil> = unterordner(&l.join(r"Mozilla\Firefox\Profiles"))
            .into_iter()
            .map(|p| p.join("cache2"))
            .filter(|p| p.is_dir())
            .filter_map(inhalt)
            .collect();
        if !ff.is_empty() {
            v.push(Ziel { browser: Some("firefox.exe"), ..ziel("Firefox-Zwischenspeicher", "Zwischengespeicherte Teile von Webseiten; Lesezeichen, Passwörter und Verlauf bleiben erhalten.", Art::Dateien(ff), false) });
        }
        if let Some(t) = inhalt(l.join("CrashDumps")) {
            v.push(ziel(
                "Absturzberichte von Programmen",
                "Speicherabbilder nach Programmabstürzen – nur für die Fehlersuche nötig.",
                Art::Dateien(vec![t]),
                false,
            ));
        }
    }
    if let Some(w) = &win {
        let mut t: Vec<Teil> = inhalt(w.join("Minidump")).into_iter().collect();
        t.push(Teil::Datei(w.join("MEMORY.DMP")));
        v.push(ziel(
            "Absturzabbilder von Windows",
            "Speicherabbilder nach Bluescreens – nur für die Fehlersuche nötig.",
            Art::Dateien(t),
            true,
        ));
    }
    if let Some(l) = &local {
        v.push(ziel(
            "Miniaturansichten",
            "Vorschaubilder im Explorer; Windows erstellt sie bei Bedarf neu.",
            Art::Dateien(vec![Teil::Muster(
                l.join(r"Microsoft\Windows\Explorer"),
                "thumbcache_",
            )]),
            false,
        ));
    }
    if win.is_some() {
        v.push(Ziel { vorgewaehlt: false, ..ziel("Komponentenspeicher", "Alte, durch Updates ersetzte Windows-Systemdateien; das kann 10 Minuten und länger dauern.", Art::Komponentenspeicher, true) });
    }
    if let Some(lw) = &lw {
        v.push(Ziel { vorgewaehlt: false, ..ziel("Ruhezustandsdatei", "Schaltet Ruhezustand und Schnellstart aus und entfernt die Datei hiberfil.sys; der PC startet danach etwas langsamer.", Art::Ruhezustand(lw.join("hiberfil.sys")), true) });
        v.push(Ziel { vorgewaehlt: false, ..ziel("Alte Windows-Version (Windows.old)", "Bleibt nach einem großen Windows-Update zurück. Danach ist die Rückkehr zur alten Windows-Version nicht mehr möglich.", Art::WindowsAlt(lw.join("Windows.old")), true) });
    }
    v
}

fn muster_dateien(dir: &Path, praefix: &str) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            n.starts_with(praefix) && n.ends_with(".db")
        })
        .collect()
}

fn datei_groesse(p: &Path) -> u64 {
    fs::symlink_metadata(p)
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .unwrap_or(0)
}

/// Belegter Platz; None = vorher nicht messbar.
pub fn messen(z: &Ziel) -> Option<u64> {
    Some(match &z.art {
        Art::Dateien(teile) => teile
            .iter()
            .map(|t| match t {
                Teil::Inhalt(p) => ordner_groesse(p),
                Teil::Datei(p) => datei_groesse(p),
                Teil::Muster(p, m) => muster_dateien(p, m).iter().map(|p| datei_groesse(p)).sum(),
            })
            .sum(),
        Art::Papierkorb => win::papierkorb_groesse(),
        Art::WindowsUpdate(p) | Art::Lieferoptimierung(p) | Art::WindowsAlt(p) => ordner_groesse(p),
        Art::Ruhezustand(p) => datei_groesse(p),
        Art::Komponentenspeicher => return None,
    })
}

/// Frei gewordener Platz auf dem Systemlaufwerk während `f`.
fn zuwachs(f: impl FnOnce()) -> u64 {
    let lw = systemlaufwerk(&|k| std::env::var(k).ok()).unwrap_or_else(|| PathBuf::from(r"C:\"));
    let frei = || win::platz(&lw.to_string_lossy()).map_or(0, |p| p.1);
    let vorher = frei();
    f();
    frei().saturating_sub(vorher)
}

pub fn ausfuehren(z: &Ziel) -> Bilanz {
    let mut b = Bilanz::default();
    match &z.art {
        Art::Dateien(teile) => {
            for t in teile {
                match t {
                    Teil::Inhalt(p) => leere_ordner(p, &mut b),
                    Teil::Datei(p) => loesche_datei_pfad(p, &mut b),
                    Teil::Muster(p, m) => muster_dateien(p, m)
                        .iter()
                        .for_each(|p| loesche_datei_pfad(p, &mut b)),
                }
            }
        }
        Art::Papierkorb => {
            let vorher = win::papierkorb_groesse();
            win::papierkorb_leeren();
            b.frei = vorher.saturating_sub(win::papierkorb_groesse());
        }
        Art::WindowsUpdate(p) => {
            // Nur wieder starten, was wir selbst gestoppt haben.
            let gestoppt = win::befehl("net.exe", &["stop", "wuauserv"]);
            leere_ordner(p, &mut b);
            if gestoppt {
                win::befehl("net.exe", &["start", "wuauserv"]);
            }
        }
        Art::Lieferoptimierung(p) => {
            let mut rest = Bilanz::default();
            let mut ps_ok = true;
            let frei = zuwachs(|| {
                let ps = r"WindowsPowerShell\v1.0\powershell.exe";
                ps_ok = win::befehl(
                    ps,
                    &[
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "Delete-DeliveryOptimizationCache -Force -ErrorAction Stop",
                    ],
                );
                if !ps_ok {
                    leere_ordner(p, &mut rest);
                }
            });
            b.frei = frei.max(rest.frei);
            b.uebersprungen = rest.uebersprungen;
            // Fehler nur, wenn auch der Ersatzweg nicht durchkam.
            b.fehler = !ps_ok && rest.uebersprungen > 0;
        }
        Art::Komponentenspeicher => {
            b.frei = zuwachs(|| {
                b.fehler = !win::befehl(
                    "dism.exe",
                    &["/Online", "/Cleanup-Image", "/StartComponentCleanup"],
                )
            })
        }
        Art::Ruhezustand(_) => {
            b.frei = zuwachs(|| b.fehler = !win::befehl("powercfg.exe", &["/h", "off"]))
        }
        Art::WindowsAlt(_) => {
            // Offizieller Weg: Datenträgerbereinigung nur mit der Kategorie "Vorherige Installationen".
            let schluessel = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VolumeCaches\Previous Installations";
            // StateFlags6542 = Speichermonitor-eigener Satz für cleanmgr /sagerun; danach wieder weg.
            b.frei = zuwachs(|| {
                b.fehler = !(win::befehl(
                    "reg.exe",
                    &[
                        "add",
                        schluessel,
                        "/v",
                        "StateFlags6542",
                        "/t",
                        "REG_DWORD",
                        "/d",
                        "2",
                        "/f",
                    ],
                ) && win::befehl("cleanmgr.exe", &["/sagerun:6542"]));
                win::befehl(
                    "reg.exe",
                    &["delete", schluessel, "/v", "StateFlags6542", "/f"],
                );
            });
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::tests::testordner;

    fn env_aus(paare: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
        move |k| paare.iter().find(|(n, _)| *n == k).map(|(_, w)| w.clone())
    }

    fn titel(v: &[Ziel]) -> Vec<&str> {
        v.iter().map(|z| z.titel).collect()
    }

    #[test]
    fn ziele_aufloesen_messen_und_aufraeumen() {
        let t = testordner("ziele");
        let s = |p: &str| t.join(p).to_string_lossy().into_owned();
        for d in [
            "temp/unter",
            "win/Temp",
            r"local\Google\Chrome\User Data\Default\Cache\Cache_Data",
            r"local\Google\Chrome\User Data\Profile 1\Code Cache",
            r"local\Mozilla\Firefox\Profiles\ab.default\cache2",
            r"local\Microsoft\Windows\Explorer",
        ] {
            fs::create_dir_all(t.join(d)).unwrap();
        }
        fs::write(t.join("temp/unter/a.tmp"), vec![0u8; 40]).unwrap();
        fs::write(
            t.join(r"local\Google\Chrome\User Data\Default\Cache\Cache_Data\f_1"),
            vec![0u8; 5],
        )
        .unwrap();
        fs::write(
            t.join(r"local\Google\Chrome\User Data\Default\Login Data"),
            b"geheim",
        )
        .unwrap();
        fs::write(
            t.join(r"local\Microsoft\Windows\Explorer\thumbcache_256.db"),
            vec![0u8; 9],
        )
        .unwrap();
        fs::write(
            t.join(r"local\Microsoft\Windows\Explorer\iconcache_16.db"),
            vec![0u8; 9],
        )
        .unwrap();

        let env = env_aus(vec![
            ("TEMP", s("temp")),
            ("SystemRoot", s("win")),
            ("LOCALAPPDATA", s("local")),
            ("SystemDrive", "Q:".into()),
        ]);
        let schutz = Schutz::aus_umgebung(&env);
        let v = ziele(&env, &schutz);
        let finde = |n: &str| v.iter().find(|z| z.titel.starts_with(n)).unwrap();

        assert_eq!(
            finde("Temporäre Dateien (Ihr").art,
            Art::Dateien(vec![Teil::Inhalt(t.join("temp"))])
        );
        assert_eq!(
            finde("Temporäre Dateien (Win").art,
            Art::Dateien(vec![Teil::Inhalt(t.join("win").join("Temp"))])
        );
        let ud = t.join(r"local\Google\Chrome\User Data");
        assert_eq!(
            finde("Chrome").art,
            Art::Dateien(vec![
                Teil::Inhalt(ud.join(r"Default\Cache")),
                Teil::Inhalt(ud.join(r"Profile 1\Code Cache"))
            ])
        );
        assert_eq!(finde("Chrome").browser, Some("chrome.exe"));
        assert!(
            !titel(&v).iter().any(|n| n.starts_with("Edge")),
            "Edge ist nicht installiert"
        );
        assert_eq!(
            finde("Firefox").art,
            Art::Dateien(vec![Teil::Inhalt(
                t.join(r"local\Mozilla\Firefox\Profiles\ab.default\cache2")
            )])
        );
        assert!(!finde("Ruhezustand").vorgewaehlt);
        assert_eq!(
            finde("Ruhezustand").art,
            Art::Ruhezustand(PathBuf::from(r"Q:\hiberfil.sys"))
        );
        assert!(finde("Ruhezustand").admin);

        assert_eq!(messen(finde("Temporäre Dateien (Ihr")), Some(40));
        assert_eq!(messen(finde("Chrome")), Some(5));
        assert_eq!(messen(finde("Miniatur")), Some(9));
        assert_eq!(messen(finde("Komponenten")), None);

        let b = ausfuehren(finde("Chrome"));
        assert_eq!((b.frei, b.geloescht), (5, 1));
        assert!(
            t.join(r"local\Google\Chrome\User Data\Default\Login Data")
                .exists(),
            "Profil-Daten müssen bleiben"
        );
        let b = ausfuehren(finde("Miniatur"));
        assert_eq!(b.frei, 9);
        assert!(t
            .join(r"local\Microsoft\Windows\Explorer\iconcache_16.db")
            .exists());
        let b = ausfuehren(finde("Temporäre Dateien (Ihr"));
        assert_eq!(b.frei, 40);
        assert!(t.join("temp").exists());
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn ungueltige_umgebung_ergibt_keine_ziele() {
        for temp in [
            "",
            "C:\\",
            "C:",
            "relativ\\temp",
            "C:\\Users\\Max",
            "C:\\Windows",
            "D:\\Daten",
            "D:\\Daten\\Tempo",
        ] {
            let env = env_aus(vec![
                ("TEMP", temp.into()),
                ("USERPROFILE", "C:\\Users\\Max".into()),
                ("SystemRoot", "C:\\Windows".into()),
            ]);
            let v = ziele(&env, &Schutz::aus_umgebung(&env));
            assert!(
                !titel(&v).contains(&"Temporäre Dateien (Ihr Benutzer)"),
                "TEMP={temp:?} darf kein Ziel ergeben"
            );
        }
        for temp in ["D:\\TMP", "C:\\Users\\Max\\AppData\\Local\\Temp"] {
            let env = env_aus(vec![("TEMP", temp.into())]);
            let v = ziele(&env, &Schutz::aus_umgebung(&env));
            assert!(
                titel(&v).contains(&"Temporäre Dateien (Ihr Benutzer)"),
                "{temp}"
            );
        }
        let env = env_aus(vec![("SystemDrive", "C:\\..".into())]);
        let v = ziele(&env, &Schutz::aus_umgebung(&env));
        assert!(!titel(&v).iter().any(|n| n.starts_with("Ruhezustand")));
    }
}
