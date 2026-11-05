#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Speichermonitor: Speicherplatz finden und freimachen.

mod scan;
mod schutz;
mod win;
mod ziele;

use eframe::egui::{self, Color32, RichText};
use scan::{groesse_text as gt, Fortschritt, Knoten};
use schutz::{Bilanz, Schutz};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ziele::{Art, Ziel};

const MAX_ZEILEN: usize = 200;
const GELB: Color32 = Color32::from_rgb(200, 140, 0);
const ROT: Color32 = Color32::from_rgb(210, 60, 50);

fn main() -> eframe::Result {
    let opt = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Speichermonitor – Speicherplatz freimachen")
            .with_inner_size([980.0, 700.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(egui::IconData {
                rgba: include_bytes!("../icon64.rgba").to_vec(), // aus werkzeug\icon.ps1
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "Speichermonitor",
        opt,
        Box::new(|cc| {
            cc.egui_ctx.set_zoom_factor(1.15);
            Ok(Box::new(App::neu()))
        }),
    )
}

#[derive(PartialEq, Clone, Copy)]
enum Seite {
    Laufwerke,
    Analyse,
    Aufraeumen,
}

#[derive(Clone)]
struct Auswahl {
    pfad: PathBuf,
    groesse: u64,
    ordner: bool,
}

type Ergebnis = (Knoten, Vec<(u64, PathBuf)>);

struct Analyse {
    wurzel: PathBuf,
    fort: Arc<Fortschritt>,
    rx: Receiver<Ergebnis>,
    ergebnis: Option<Ergebnis>,
    dateien_zeigen: bool,
}

struct Eintrag {
    ziel: Ziel,
    groesse: Option<Option<u64>>, // None = wird gemessen, Some(None) = unbekannt
    an: bool,
}

enum Frage {
    Papierkorb(Auswahl),
    Aufraeumen,
    WindowsAlt,
}

struct App {
    seite: Seite,
    admin: bool,
    schutz: Schutz,
    laufwerke: Vec<win::Laufwerk>,
    analyse: Option<Analyse>,
    auswahl: Option<Auswahl>,
    eintraege: Vec<Eintrag>,
    mess_rx: Option<Receiver<(usize, Option<u64>)>>,
    lauf: Option<(Arc<Mutex<String>>, Receiver<String>)>,
    meldung: Option<String>,
    frage: Option<Frage>,
    /// Laufende Browser, höchstens 1×/s neu ermittelt (Prozess-Snapshot ist teuer).
    browser_offen: Vec<&'static str>,
    browser_zeit: Option<Instant>,
}

impl App {
    fn neu() -> App {
        let schutz = Schutz::system();
        let eintraege = ziele::ziele(&|k| std::env::var(k).ok(), &schutz)
            .into_iter()
            .map(|z| Eintrag {
                an: z.vorgewaehlt,
                ziel: z,
                groesse: None,
            })
            .collect();
        let mut app = App {
            seite: Seite::Laufwerke,
            admin: win::ist_admin(),
            schutz,
            laufwerke: win::laufwerke(),
            analyse: None,
            auswahl: None,
            eintraege,
            mess_rx: None,
            lauf: None,
            meldung: None,
            frage: None,
            browser_offen: Vec::new(),
            browser_zeit: None,
        };
        app.messen();
        app
    }

    fn messen(&mut self) {
        let ziele: Vec<Ziel> = self.eintraege.iter().map(|e| e.ziel.clone()).collect();
        self.eintraege.iter_mut().for_each(|e| e.groesse = None);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (i, z) in ziele.iter().enumerate() {
                if tx.send((i, ziele::messen(z))).is_err() {
                    return;
                }
            }
        });
        self.mess_rx = Some(rx);
    }

    fn analysieren(&mut self, wurzel: PathBuf) {
        if let Some(a) = &self.analyse {
            a.fort.abbrechen.store(true, Relaxed);
        }
        let fort = Arc::new(Fortschritt::default());
        let (tx, rx) = channel();
        let (f, w) = (fort.clone(), wurzel.clone());
        std::thread::spawn(move || {
            let k = scan::scanne(&w, &f);
            let _ = tx.send((k, f.groesste()));
        });
        self.analyse = Some(Analyse {
            wurzel,
            fort,
            rx,
            ergebnis: None,
            dateien_zeigen: false,
        });
        self.auswahl = None;
        self.seite = Seite::Analyse;
    }

    fn darf_weg(&self, p: &Path) -> Result<(), &'static str> {
        self.schutz.pruefe_papierkorb(p)
    }

    fn papierkorb(&mut self, a: Auswahl) {
        if self.darf_weg(&a.pfad).is_err() || !win::in_papierkorb(&a.pfad) {
            self.meldung = Some(format!(
                "„{}“ konnte nicht in den Papierkorb verschoben werden.",
                a.pfad.display()
            ));
            return;
        }
        if let Some(Analyse {
            wurzel,
            ergebnis: Some((baum, top)),
            ..
        }) = &mut self.analyse
        {
            if let Ok(rel) = a.pfad.strip_prefix(&*wurzel) {
                let teile: Vec<String> = rel
                    .iter()
                    .map(|t| t.to_string_lossy().into_owned())
                    .collect();
                baum.entferne(&teile, a.groesse, a.ordner);
            }
            top.retain(|(_, p)| !p.starts_with(&a.pfad));
        }
        self.meldung = Some(format!(
            "{} in den Papierkorb verschoben. Der Platz wird erst frei, wenn der Papierkorb geleert ist (Reiter Aufräumen).",
            gt(a.groesse)
        ));
        self.auswahl = None;
        self.laufwerke = win::laufwerke();
    }

    fn aufraeumen(&mut self) {
        let admin = self.admin;
        let ziele: Vec<Ziel> = self
            .eintraege
            .iter()
            .filter(|e| {
                e.an && (admin || !e.ziel.admin) && !matches!(e.ziel.art, Art::WindowsAlt(_))
            })
            .map(|e| e.ziel.clone())
            .collect();
        self.starte(ziele);
    }

    fn starte(&mut self, ziele: Vec<Ziel>) {
        let status = Arc::new(Mutex::new(String::new()));
        let (tx, rx) = channel();
        let st = status.clone();
        std::thread::spawn(move || {
            let mut b = Bilanz::default();
            let mut offen = Vec::new();
            let mut fehl = Vec::new();
            for z in &ziele {
                if z.browser.is_some_and(win::laeuft) {
                    offen.push(z.titel);
                    continue;
                }
                *st.lock().unwrap() = format!("Wird aufgeräumt: {} …", z.titel);
                let r = ziele::ausfuehren(z);
                if r.fehler {
                    fehl.push(format!("„{}“ konnte nicht ausgeführt werden.", z.titel));
                }
                b.plus(r);
            }
            let mut t = format!("{} frei gemacht", gt(b.frei));
            if b.uebersprungen > 0 {
                t += &format!(", {} Dateien übersprungen (in Gebrauch)", b.uebersprungen);
            }
            t += ".";
            if !offen.is_empty() {
                t += &format!(
                    " Nicht aufgeräumt, weil der Browser noch offen ist: {}.",
                    offen.join(", ")
                );
            }
            for f in fehl {
                t += " ";
                t += &f;
            }
            let _ = tx.send(t);
        });
        self.lauf = Some((status, rx));
    }

    fn abholen(&mut self, ctx: &egui::Context) {
        let mut weiter = false;
        if let Some(rx) = &self.mess_rx {
            while let Ok((i, g)) = rx.try_recv() {
                self.eintraege[i].groesse = Some(g);
            }
            weiter |= self.eintraege.iter().any(|e| e.groesse.is_none());
        }
        if let Some(a) = &mut self.analyse {
            if let Ok(e) = a.rx.try_recv() {
                a.ergebnis = Some(e);
            }
            weiter |= a.ergebnis.is_none();
        }
        if let Some((_, rx)) = &self.lauf {
            if let Ok(t) = rx.try_recv() {
                self.meldung = Some(t);
                self.lauf = None;
                self.laufwerke = win::laufwerke();
                self.messen();
            }
            weiter = true;
        }
        if weiter {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.abholen(&ctx);

        egui::Panel::top("kopf").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Speichermonitor");
                ui.add_space(16.0);
                for (s, t) in [
                    (Seite::Laufwerke, "Laufwerke"),
                    (Seite::Analyse, "Was belegt Platz?"),
                    (Seite::Aufraeumen, "Aufräumen"),
                ] {
                    if ui
                        .selectable_label(self.seite == s, RichText::new(t).size(16.0))
                        .clicked()
                    {
                        self.seite = s;
                    }
                }
            });
            if !self.admin {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Einige Aufräum-Schritte brauchen Administratorrechte.")
                            .color(GELB),
                    );
                    if ui.button("Als Administrator neu starten").clicked()
                        && win::als_admin_neu_starten()
                    {
                        std::process::exit(0);
                    }
                });
            }
            ui.add_space(4.0);
        });

        if let Some(m) = self.meldung.clone() {
            egui::Panel::bottom("meldung").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(m).strong());
                    if ui.small_button("OK").clicked() {
                        self.meldung = None;
                    }
                });
            });
        }

        egui::CentralPanel::default_margins().show(ui, |ui| match self.seite {
            Seite::Laufwerke => self.seite_laufwerke(ui),
            Seite::Analyse => self.seite_analyse(ui),
            Seite::Aufraeumen => self.seite_aufraeumen(ui),
        });

        self.fragen(&ctx);
    }
}

fn balken(ui: &mut egui::Ui, anteil: f32, breite: f32) {
    let farbe = if anteil > 0.9 {
        ROT
    } else {
        Color32::from_rgb(60, 130, 210)
    };
    ui.add(
        egui::ProgressBar::new(anteil)
            .desired_width(breite)
            .fill(farbe),
    );
}

impl App {
    fn seite_laufwerke(&mut self, ui: &mut egui::Ui) {
        ui.label("Klicken Sie auf ein Laufwerk, um zu sehen, was dort Platz belegt.");
        ui.add_space(8.0);
        let mut ziel = None;
        for l in &self.laufwerke {
            let belegt = l.groesse - l.frei;
            let anteil = belegt as f32 / l.groesse as f32;
            let r = egui::Frame::group(ui.style())
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}  {}", &l.wurzel[..2], l.name))
                                .size(17.0)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Analysieren").clicked() {
                                ziel = Some(PathBuf::from(&l.wurzel));
                            }
                        });
                    });
                    balken(ui, anteil, ui.available_width());
                    ui.label(format!(
                        "{} frei von {}  ·  {} belegt",
                        gt(l.frei),
                        gt(l.groesse),
                        gt(belegt)
                    ));
                })
                .response
                .interact(egui::Sense::click());
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ziel = Some(PathBuf::from(&l.wurzel));
            }
            ui.add_space(6.0);
        }
        if ui.button("Neu einlesen").clicked() {
            self.laufwerke = win::laufwerke();
        }
        if let Some(z) = ziel {
            self.analysieren(z);
        }
    }

    fn seite_analyse(&mut self, ui: &mut egui::Ui) {
        // Herausnehmen, damit beim Zeichnen self frei bleibt; danach zurück.
        let Some(mut a) = self.analyse.take() else {
            ui.label("Wählen Sie zuerst unter „Laufwerke“ ein Laufwerk aus.");
            return;
        };
        if self.analyse_zeigen(ui, &mut a) {
            self.analysieren(a.wurzel);
        } else {
            self.analyse = Some(a);
        }
    }

    /// true = neu untersuchen.
    fn analyse_zeigen(&mut self, ui: &mut egui::Ui, a: &mut Analyse) -> bool {
        let Some((baum, top)) = &a.ergebnis else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{} wird untersucht … {} Dateien, {}",
                    a.wurzel.display(),
                    a.fort.dateien.load(Relaxed),
                    gt(a.fort.bytes.load(Relaxed))
                ));
                if ui.button("Abbrechen").clicked() {
                    a.fort.abbrechen.store(true, Relaxed);
                }
            });
            return false;
        };
        let mut neu = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} – {} belegt", a.wurzel.display(), gt(baum.groesse))).strong());
            if a.fort.abbrechen.load(Relaxed) {
                ui.label(RichText::new("(abgebrochen – unvollständig)").color(GELB));
            }
            let fehler = a.fort.fehler.load(Relaxed);
            if fehler > 0 {
                ui.label(RichText::new(format!("{fehler} Ordner ohne Zugriff")).weak())
                    .on_hover_text("Diese Ordner darf Speichermonitor ohne Administratorrechte nicht lesen. Sie fehlen in der Rechnung.");
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut a.dateien_zeigen, false, "Ordner");
            ui.selectable_value(&mut a.dateien_zeigen, true, "Größte Dateien");
            neu = ui.button("Neu untersuchen").clicked();
        });
        let wurzel = &a.wurzel;
        let dateien_zeigen = a.dateien_zeigen;

        egui::Panel::bottom("auswahl").show(ui, |ui| {
            ui.add_space(4.0);
            match self.auswahl.clone() {
                None => _ = ui.label(
                    RichText::new(
                        "Tipp: Eintrag anklicken oder mit rechts anklicken für weitere Schritte.",
                    )
                    .weak(),
                ),
                Some(s) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}  ({})", s.pfad.display(), gt(s.groesse)));
                    });
                    ui.horizontal(|ui| self.knoepfe(ui, &s));
                }
            }
            ui.add_space(4.0);
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if dateien_zeigen {
                    for (g, p) in top {
                        let s = Auswahl {
                            pfad: p.clone(),
                            groesse: *g,
                            ordner: false,
                        };
                        ui.horizontal(|ui| {
                            ui.add_sized([90.0, 18.0], egui::Label::new(gt(*g)));
                            let r = ui
                                .selectable_label(self.ist_ausgewaehlt(p), p.display().to_string());
                            self.zeile_reaktion(r, s);
                        });
                    }
                } else {
                    self.baum(ui, baum, wurzel);
                }
            });
        neu
    }

    fn ist_ausgewaehlt(&self, p: &Path) -> bool {
        self.auswahl.as_ref().is_some_and(|a| a.pfad == p)
    }

    fn zeile_reaktion(&mut self, r: egui::Response, s: Auswahl) {
        if r.clicked() || r.secondary_clicked() {
            self.auswahl = Some(s.clone());
        }
        r.context_menu(|ui| self.knoepfe(ui, &s));
    }

    fn knoepfe(&mut self, ui: &mut egui::Ui, s: &Auswahl) {
        if ui.button("Im Explorer zeigen").clicked() {
            win::im_explorer_zeigen(&s.pfad);
            ui.close();
        }
        let sperre = self.darf_weg(&s.pfad);
        let r = ui.add_enabled(sperre.is_ok(), egui::Button::new("In den Papierkorb …"));
        if let Err(grund) = sperre {
            r.on_disabled_hover_text(grund);
        } else if r.clicked() {
            self.frage = Some(Frage::Papierkorb(s.clone()));
            ui.close();
        }
    }

    fn baum(&mut self, ui: &mut egui::Ui, k: &Knoten, pfad: &Path) {
        let zeile = |ui: &mut egui::Ui, app: &App, name: &str, g: u64, p: &Path| {
            let anteil = if k.groesse > 0 {
                g as f32 / k.groesse as f32
            } else {
                0.0
            };
            ui.add_sized([80.0, 18.0], egui::Label::new(gt(g)));
            ui.add(
                egui::ProgressBar::new(anteil)
                    .desired_width(90.0)
                    .text(format!("{:.0} %", anteil * 100.0)),
            );
            ui.selectable_label(app.ist_ausgewaehlt(p), name)
        };
        for kind in k.kinder.iter().take(MAX_ZEILEN) {
            let p = pfad.join(&kind.name);
            let s = Auswahl {
                pfad: p.clone(),
                groesse: kind.groesse,
                ordner: true,
            };
            if kind.kinder.is_empty() {
                let r = ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    zeile(ui, self, &kind.name, kind.groesse, &p)
                });
                self.zeile_reaktion(r.inner, s);
            } else {
                let id = ui.make_persistent_id(&p);
                let (_, kopf, _) =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        id,
                        false,
                    )
                    .show_header(ui, |ui| zeile(ui, self, &kind.name, kind.groesse, &p))
                    .body(|ui| self.baum(ui, kind, &p));
                self.zeile_reaktion(kopf.inner, s);
            }
        }
        if k.kinder.len() > MAX_ZEILEN {
            ui.label(
                RichText::new(format!(
                    "… und {} kleinere Ordner",
                    k.kinder.len() - MAX_ZEILEN
                ))
                .weak(),
            );
        }
        if k.dateien > 0 {
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().indent);
                let anteil = k.dateien as f32 / k.groesse.max(1) as f32;
                ui.add_sized([80.0, 18.0], egui::Label::new(gt(k.dateien)));
                ui.add(
                    egui::ProgressBar::new(anteil)
                        .desired_width(90.0)
                        .text(format!("{:.0} %", anteil * 100.0)),
                );
                ui.label(RichText::new("Dateien direkt in diesem Ordner").italics());
            });
        }
    }

    fn seite_aufraeumen(&mut self, ui: &mut egui::Ui) {
        if let Some((status, _)) = &self.lauf {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(status.lock().unwrap().clone()).size(16.0));
            });
            return;
        }
        ui.label("Haken setzen und auf „Ausgewählte aufräumen“ klicken. Ihre eigenen Dateien (Dokumente, Bilder usw.) werden nicht angefasst – nur „Papierkorb leeren“ löscht endgültig, was Sie selbst in den Papierkorb gelegt haben.");
        ui.add_space(6.0);
        if self
            .browser_zeit
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.browser_offen = self
                .eintraege
                .iter()
                .filter_map(|e| e.ziel.browser)
                .filter(|b| win::laeuft(b))
                .collect();
            self.browser_zeit = Some(Instant::now());
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let browser_offen = &self.browser_offen;
        let admin = self.admin;
        let mut windows_alt = false;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).max_height(ui.available_height() - 50.0).show(ui, |ui| {
            for e in &mut self.eintraege {
                let gesperrt = e.ziel.admin && !admin;
                let alt = matches!(e.ziel.art, Art::WindowsAlt(_));
                if matches!(&e.ziel.art, Art::WindowsAlt(p) if !p.exists()) {
                    continue;
                }
                ui.horizontal(|ui| {
                    if alt {
                        if ui.add_enabled(!gesperrt && e.groesse.is_some(), egui::Button::new("Entfernen …")).clicked() {
                            windows_alt = true;
                        }
                    } else {
                        let mut an = e.an && !gesperrt;
                        if ui.add_enabled(!gesperrt, egui::Checkbox::without_text(&mut an)).changed() {
                            e.an = an;
                        }
                    }
                    ui.label(RichText::new(e.ziel.titel).size(16.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match e.groesse {
                        None => _ = ui.spinner(),
                        Some(None) => _ = ui.label("Größe erst nach dem Aufräumen bekannt"),
                        Some(Some(g)) => _ = ui.label(RichText::new(gt(g)).size(16.0)),
                    });
                });
                ui.indent(e.ziel.titel, |ui| {
                    ui.label(RichText::new(e.ziel.erklaerung).weak());
                    if gesperrt {
                        ui.label(RichText::new("Braucht Administratorrechte – oben „Als Administrator neu starten“.").color(GELB));
                    }
                    if e.ziel.browser.is_some_and(|b| browser_offen.contains(&b)) {
                        ui.label(RichText::new("Browser ist noch geöffnet – bitte schließen, sonst wird er übersprungen.").color(GELB));
                    }
                });
                ui.separator();
            }
        });
        let summe: u64 = self
            .eintraege
            .iter()
            .filter(|e| {
                e.an && (admin || !e.ziel.admin) && !matches!(e.ziel.art, Art::WindowsAlt(_))
            })
            .filter_map(|e| e.groesse.flatten())
            .sum();
        let irgendwas = self
            .eintraege
            .iter()
            .any(|e| e.an && (admin || !e.ziel.admin) && !matches!(e.ziel.art, Art::WindowsAlt(_)));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    irgendwas,
                    egui::Button::new(RichText::new("Ausgewählte aufräumen").size(17.0).strong()),
                )
                .clicked()
            {
                self.frage = Some(Frage::Aufraeumen);
            }
            ui.label(format!("ca. {} werden frei", gt(summe)));
            if ui.button("Neu messen").clicked() {
                self.messen();
            }
        });
        if windows_alt {
            self.frage = Some(Frage::WindowsAlt);
        }
    }

    fn fragen(&mut self, ctx: &egui::Context) {
        let Some(frage) = &self.frage else { return };
        let mut antwort = None;
        egui::Modal::new(egui::Id::new("frage")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            let ja = match frage {
                Frage::Papierkorb(s) => {
                    ui.heading("In den Papierkorb verschieben?");
                    ui.add_space(6.0);
                    ui.label(RichText::new(s.pfad.display().to_string()).strong());
                    ui.label(format!("Größe: {}", gt(s.groesse)));
                    ui.label("Sie können es aus dem Papierkorb wiederherstellen, solange er nicht geleert wird.");
                    "In den Papierkorb"
                }
                Frage::Aufraeumen => {
                    ui.heading("Jetzt aufräumen?");
                    ui.add_space(6.0);
                    for e in self.eintraege.iter().filter(|e| e.an && (self.admin || !e.ziel.admin) && !matches!(e.ziel.art, Art::WindowsAlt(_))) {
                        ui.label(format!("•  {}  ({})", e.ziel.titel, e.groesse.flatten().map(gt).unwrap_or_else(|| "?".into())));
                    }
                    ui.label("Diese Dateien werden endgültig gelöscht (nicht in den Papierkorb).");
                    "Jetzt aufräumen"
                }
                Frage::WindowsAlt => {
                    ui.heading("Alte Windows-Version entfernen?");
                    ui.add_space(6.0);
                    ui.label(RichText::new("Die Rückkehr zur alten Windows-Version ist danach nicht mehr möglich.").strong().color(ROT));
                    ui.label("Windows entfernt den Ordner Windows.old mit der Datenträgerbereinigung. Das kann einige Minuten dauern.");
                    "Endgültig entfernen"
                }
            };
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(ja).strong()).clicked() {
                    antwort = Some(true);
                }
                if ui.button("Abbrechen").clicked() {
                    antwort = Some(false);
                }
            });
        });
        let Some(ja) = antwort else { return };
        let frage = self.frage.take().unwrap();
        if !ja {
            return;
        }
        match frage {
            Frage::Papierkorb(s) => self.papierkorb(s),
            Frage::Aufraeumen => self.aufraeumen(),
            Frage::WindowsAlt => {
                let z: Vec<Ziel> = self
                    .eintraege
                    .iter()
                    .filter(|e| matches!(e.ziel.art, Art::WindowsAlt(_)))
                    .map(|e| e.ziel.clone())
                    .collect();
                self.starte(z);
            }
        }
    }
}
