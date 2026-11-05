//! Paralleler Ordner-Scan: Größen je Ordner plus die größten Dateien.

use rayon::prelude::*;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

pub const ATTR_ORDNER: u32 = 0x10;
pub const ATTR_REPARSE: u32 = 0x400;
// OneDrive-Platzhalter u. ä.: die Datei liegt nicht auf der Platte.
const ATTR_NICHT_LOKAL: u32 = 0x0004_0000 | 0x0040_0000;
pub const TAG_SYMLINK: u32 = 0xA000_000C;
pub const TAG_MOUNT_POINT: u32 = 0xA000_0003; // auch Junctions
pub const TOP: usize = 100;

/// "1,5 GB" – Basis 1024 wie im Explorer.
pub fn groesse_text(b: u64) -> String {
    const EINHEIT: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < EINHEIT.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", EINHEIT[i]).replace('.', ",")
}

#[derive(Debug, Default)]
pub struct Knoten {
    pub name: String,
    pub groesse: u64,
    /// Summe der Dateien direkt in diesem Ordner.
    pub dateien: u64,
    /// Unterordner, größte zuerst.
    pub kinder: Vec<Knoten>,
}

impl Knoten {
    /// Nimmt einen gelöschten Eintrag (Pfadteile ab diesem Knoten) aus dem Baum
    /// und zieht seine Größe bei allen Eltern ab. Gibt die abgezogene Größe zurück.
    pub fn entferne(&mut self, teile: &[String], groesse: u64, ist_ordner: bool) -> Option<u64> {
        let weg = match teile {
            [] => return None,
            [name] if ist_ordner => {
                let i = self.kinder.iter().position(|k| &k.name == name)?;
                self.kinder.remove(i).groesse
            }
            [_] => {
                let g = groesse.min(self.dateien);
                self.dateien -= g;
                g
            }
            [erster, rest @ ..] => self
                .kinder
                .iter_mut()
                .find(|k| &k.name == erster)?
                .entferne(rest, groesse, ist_ordner)?,
        };
        self.groesse = self.groesse.saturating_sub(weg);
        Some(weg)
    }
}

#[derive(Default)]
pub struct Fortschritt {
    pub abbrechen: AtomicBool,
    pub dateien: AtomicU64,
    pub bytes: AtomicU64,
    pub fehler: AtomicU64,
    top: Mutex<Vec<(u64, PathBuf)>>,
    top_min: AtomicU64,
}

impl Fortschritt {
    fn merke(&self, g: u64, pfad: impl FnOnce() -> PathBuf) {
        if g <= self.top_min.load(Relaxed) {
            return;
        }
        let mut t = self.top.lock().unwrap();
        t.push((g, pfad()));
        if t.len() >= TOP * 2 {
            t.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
            t.truncate(TOP);
            self.top_min.store(t[TOP - 1].0, Relaxed);
        }
    }

    pub fn groesste(&self) -> Vec<(u64, PathBuf)> {
        let mut t = self.top.lock().unwrap().clone();
        t.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
        t.truncate(TOP);
        t
    }
}

/// Scannt `pfad` rekursiv. Junctions/Symlinks werden nie verfolgt,
/// Zugriffsfehler nur gezählt.
pub fn scanne(pfad: &Path, f: &Fortschritt) -> Knoten {
    let name = pfad
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| pfad.display().to_string());
    let mut k = Knoten {
        name,
        ..Default::default()
    };
    if f.abbrechen.load(Relaxed) {
        return k;
    }
    let Ok(rd) = std::fs::read_dir(pfad) else {
        f.fehler.fetch_add(1, Relaxed);
        return k;
    };
    let mut ordner = Vec::new();
    for e in rd {
        // DirEntry::metadata folgt unter Windows keinen Verknüpfungen.
        let Some((e, m)) = e.ok().and_then(|e| e.metadata().ok().map(|m| (e, m))) else {
            f.fehler.fetch_add(1, Relaxed);
            continue;
        };
        let attr = m.file_attributes();
        if attr & ATTR_ORDNER != 0 {
            // Nur Symlinks/Junctions auslassen; andere Reparse-Ordner (z. B. OneDrive) zählen.
            // Unbekannter Tag = auslassen.
            if attr & ATTR_REPARSE == 0
                || crate::win::reparse_tag(&e.path())
                    .is_some_and(|t| t != TAG_SYMLINK && t != TAG_MOUNT_POINT)
            {
                ordner.push(e.path());
            }
            continue;
        }
        let g = if attr & ATTR_NICHT_LOKAL != 0 {
            0
        } else {
            m.len()
        };
        k.dateien += g;
        f.dateien.fetch_add(1, Relaxed);
        f.bytes.fetch_add(g, Relaxed);
        f.merke(g, || e.path());
    }
    k.kinder = ordner.par_iter().map(|p| scanne(p, f)).collect();
    k.groesse = k.dateien + k.kinder.iter().map(|c| c.groesse).sum::<u64>();
    k.kinder
        .sort_unstable_by_key(|c| std::cmp::Reverse(c.groesse));
    k
}

/// Gesamtgröße eines Ordners (0, wenn es ihn nicht gibt).
pub fn ordner_groesse(pfad: &Path) -> u64 {
    if !pfad.is_dir() {
        return 0;
    }
    scanne(pfad, &Fortschritt::default()).groesse
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::fs;

    pub fn testordner(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "speichermonitor-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn junction(link: &Path, ziel: &Path) {
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(ziel)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J fehlgeschlagen");
    }

    #[test]
    fn groessen_format() {
        assert_eq!(groesse_text(0), "0 B");
        assert_eq!(groesse_text(1023), "1023 B");
        assert_eq!(groesse_text(1024), "1,0 KB");
        assert_eq!(groesse_text(1536 * 1024 * 1024), "1,5 GB");
        assert_eq!(groesse_text(3 << 40), "3,0 TB");
    }

    #[test]
    fn baum_summiert_und_folgt_keiner_junction() {
        let t = testordner("scan");
        let w = t.join("wurzel");
        fs::create_dir_all(w.join("a/tief")).unwrap();
        fs::create_dir_all(w.join("b")).unwrap();
        fs::create_dir_all(t.join("aussen")).unwrap();
        fs::write(w.join("x.bin"), vec![0u8; 10]).unwrap();
        fs::write(w.join("a/y.bin"), vec![0u8; 100]).unwrap();
        fs::write(w.join("a/tief/z.bin"), vec![0u8; 1000]).unwrap();
        fs::write(w.join("b/q.bin"), vec![0u8; 50]).unwrap();
        fs::write(t.join("aussen/gross.bin"), vec![0u8; 5000]).unwrap();
        junction(&w.join("link"), &t.join("aussen"));
        assert_eq!(
            crate::win::reparse_tag(&w.join("link")),
            Some(TAG_MOUNT_POINT)
        );
        assert_eq!(crate::win::reparse_tag(&w.join("a")), Some(0));

        let f = Fortschritt::default();
        let mut k = scanne(&w, &f);
        assert_eq!(k.groesse, 1160, "Junction darf nicht mitgezählt werden");
        assert_eq!(k.dateien, 10);
        assert_eq!(k.kinder.len(), 2);
        assert_eq!(k.kinder[0].name, "a");
        assert_eq!(k.kinder[0].groesse, 1100);
        assert_eq!(f.groesste()[0].0, 1000);
        assert_eq!(f.groesste().len(), 4);

        // Datei abziehen, dann Ordner entfernen
        assert_eq!(
            k.entferne(&["a".into(), "tief".into(), "z.bin".into()], 1000, false),
            Some(1000)
        );
        assert_eq!((k.groesse, k.kinder[0].groesse), (160, 100));
        assert_eq!(k.entferne(&["b".into()], 0, true), Some(50));
        assert_eq!(k.groesse, 110);
        assert_eq!(k.entferne(&["gibtsnicht".into()], 0, true), None);
        fs::remove_dir_all(&t).unwrap();
    }
}
