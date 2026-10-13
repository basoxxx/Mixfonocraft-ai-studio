//! Salvataggio automatico e recupero dopo un crash.
//!
//! - Ogni [`INTERVAL`], se il progetto è modificato, una copia viene scritta in
//!   `<config>/autosave/<titolo>.mfc` (scrittura atomica: file temporaneo + rinomina).
//!   Restano al massimo [`MAX_FILES`] copie, le più vecchie vengono eliminate.
//! - Un file `<config>/session.lock` segnala una sessione aperta. Se esiste già all'avvio,
//!   la sessione precedente è terminata senza chiudersi: si propone il recupero.
//! - Un salvataggio normale elimina la copia automatica del progetto ([`discard_for_title`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mfc_core::Project;

use crate::plugins::config_dir;
use crate::state::Studio;

/// Intervallo tra due salvataggi automatici di un progetto modificato.
pub const INTERVAL: Duration = Duration::from_secs(120);
/// Numero massimo di copie automatiche conservate.
pub const MAX_FILES: usize = 10;

const SENZA_TITOLO: &str = "senza_titolo";
const EXT: &str = "mfc";
const DIR_NAME: &str = "autosave";
const LOCK_NAME: &str = "session.lock";
const RECOVERY_TITLE: &str = "Ripristinare il progetto non salvato?";

/// Stato dell'autosalvataggio, tenuto in [`crate::app::StudioApp`].
///
/// Di default è inattivo: [`Autosave::start`] lo attiva (non viene fatto nei test).
#[derive(Default)]
pub struct Autosave {
    dir: Option<PathBuf>,
    lock: Option<PathBuf>,
    last_check: Option<Instant>,
    /// Copie da proporre per il recupero (più recenti prima), se la sessione precedente è caduta.
    recovery: Option<Vec<(PathBuf, SystemTime)>>,
}

impl Drop for Autosave {
    fn drop(&mut self) {
        // Uscita pulita: la sessione non è più "aperta".
        if let Some(lock) = self.lock.take() {
            let _ = fs::remove_file(lock);
        }
    }
}

impl Autosave {
    /// Avvia l'autosalvataggio nella cartella di configurazione dell'app.
    pub fn start(&mut self) {
        let (Some(dir), Some(lock)) = (autosave_dir(), lock_path()) else {
            return;
        };
        self.start_in(dir, lock);
    }

    /// Come [`Autosave::start`], con percorsi espliciti (usato dai test).
    pub fn start_in(&mut self, dir: PathBuf, lock: PathBuf) {
        let _ = fs::create_dir_all(&dir);
        if lock.exists() {
            // Sessione precedente non chiusa correttamente.
            let files = newest_first(list_autosaves(&dir));
            if !files.is_empty() {
                self.recovery = Some(files);
            }
        }
        if let Some(parent) = lock.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&lock, format!("{}\n", std::process::id()));
        self.dir = Some(dir);
        self.lock = Some(lock);
        self.last_check = Some(Instant::now());
    }

    /// Lavoro per frame: autosalvataggio periodico e finestra di recupero.
    pub fn tick(&mut self, studio: &mut Studio, ctx: &egui::Context) {
        self.autosave_if_due(studio);
        self.recovery_dialog(studio, ctx);
    }

    fn autosave_if_due(&mut self, studio: &mut Studio) {
        let Some(dir) = self.dir.clone() else { return };
        // Il conto alla rovescia parte dalla prima modifica: un progetto pulito non viene copiato.
        let now = Instant::now();
        if !studio.dirty {
            self.last_check = Some(now);
            return;
        }
        let due = self.last_check.is_none_or(|t| now.duration_since(t) >= INTERVAL);
        if !due {
            return;
        }
        self.last_check = Some(now);
        studio.sync_plugin_states();
        let result = studio
            .project
            .to_json()
            .map_err(|e| e.to_string())
            .and_then(|json| write_autosave(&dir, &studio.project.info.title, &json).map_err(|e| e.to_string()));
        match result {
            Ok(_) => prune(&dir, MAX_FILES),
            Err(e) => studio.status = format!("Salvataggio automatico fallito: {e}"),
        }
    }

    fn recovery_dialog(&mut self, studio: &mut Studio, ctx: &egui::Context) {
        let Some(files) = self.recovery.as_ref() else { return };
        let mut chosen = None;
        let mut ignore = false;
        egui::Window::new(RECOVERY_TITLE)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Mixfonocraft non si è chiuso correttamente. Sono disponibili queste copie automatiche:");
                ui.add_space(6.0);
                for (path, when) in files {
                    ui.horizontal(|ui| {
                        ui.label(
                            path.file_stem()
                                .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
                        );
                        ui.weak(format_utc(*when));
                        if ui.button("Apri").clicked() {
                            chosen = Some(path.clone());
                        }
                    });
                }
                ui.add_space(8.0);
                ignore = ui.button("Ignora").clicked();
            });
        if let Some(path) = chosen {
            self.recovery = None;
            recover(studio, &path);
        } else if ignore {
            self.recovery = None;
        }
    }
}

/// Apre una copia automatica. Il progetto recuperato non ha percorso: al primo salvataggio
/// l'utente sceglie dove conservarlo, così la copia automatica non viene mai aperta come file.
fn recover(studio: &mut Studio, path: &Path) {
    let parsed = fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|txt| Project::from_json(&txt).map_err(|e| e.to_string()));
    match parsed {
        Ok(project) => {
            studio.replace_project(project, None);
            studio.dirty = true;
            studio.status = "Progetto recuperato: salvalo per conservarlo".into();
        }
        Err(e) => studio.status = format!("Recupero fallito: {e}"),
    }
}

/// Elimina la copia automatica del progetto dopo un salvataggio normale.
pub fn discard_for_title(title: &str) {
    if let Some(dir) = autosave_dir() {
        let _ = fs::remove_file(dir.join(file_name_for(title)));
    }
}

fn autosave_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join(DIR_NAME))
}

fn lock_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(LOCK_NAME))
}

/// Nome senza estensione derivato dal titolo: caratteri non validi sostituiti, mai vuoto.
pub fn file_stem_for(title: &str) -> String {
    let clean: String = title
        .chars()
        .take(80)
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if clean.is_empty() {
        SENZA_TITOLO.to_string()
    } else {
        clean.to_string()
    }
}

/// Nome del file di autosalvataggio per il titolo indicato, es. `Brano.mfc`.
pub fn file_name_for(title: &str) -> String {
    format!("{}.{EXT}", file_stem_for(title))
}

/// Dai file da conservare: restituisce quelli da eliminare (tutti oltre i `keep` più recenti).
pub fn prune_plan(entries: Vec<(PathBuf, SystemTime)>, keep: usize) -> Vec<PathBuf> {
    newest_first(entries).into_iter().skip(keep).map(|(p, _)| p).collect()
}

/// Ordina per data decrescente (a parità, per nome) così che la prima voce sia la più recente.
pub fn newest_first(mut entries: Vec<(PathBuf, SystemTime)>) -> Vec<(PathBuf, SystemTime)> {
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
}

/// Data UTC leggibile, es. `13/10/2026 08:30 UTC`.
pub fn format_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{d:02}/{m:02}/{y} {:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60)
}

/// Giorni dal 1970-01-01 → (anno, mese, giorno). Algoritmo di H. Hinnant.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Copie `.mfc` presenti nella cartella, con la data di modifica.
fn list_autosaves(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == EXT) && p.is_file())
        .filter_map(|p| {
            let modified = fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, modified))
        })
        .collect()
}

/// Scrive `json` in `<dir>/<titolo>.mfc` tramite file temporaneo e rinomina.
fn write_autosave(dir: &Path, title: &str, json: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = file_name_for(title);
    let tmp = dir.join(format!(".{name}.tmp"));
    let dest = dir.join(&name);
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Elimina le copie più vecchie oltre `keep`. Gli errori sono ignorati: è pulizia best-effort.
fn prune(dir: &Path, keep: usize) {
    for old in prune_plan(list_autosaves(dir), keep) {
        let _ = fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cartella temporanea univoca, rimossa a fine test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mfc-autosave-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn nomi_file_sanificati() {
        assert_eq!(file_name_for("Brano nuovo"), "Brano nuovo.mfc");
        assert_eq!(file_name_for("a/b:c*?"), "a_b_c__.mfc");
        assert_eq!(file_name_for("   "), "senza_titolo.mfc");
        assert_eq!(file_name_for(".."), "senza_titolo.mfc");
        assert_eq!(file_name_for(".nascosto"), "nascosto.mfc");
        assert_eq!(file_stem_for(&"x".repeat(200)).chars().count(), 80);
    }

    #[test]
    fn data_utc() {
        assert_eq!(format_utc(at(0)), "01/01/1970 00:00 UTC");
        // 2000-03-01 00:00 UTC (anno bisestile).
        assert_eq!(format_utc(at(11_017 * 86_400)), "01/03/2000 00:00 UTC");
        assert_eq!(format_utc(at(1_781_000_000)), "09/06/2026 10:13 UTC");
    }

    #[test]
    fn pruning_tiene_le_piu_recenti() {
        let entries: Vec<_> = (0..12u64)
            .map(|i| (PathBuf::from(format!("f{i:02}.mfc")), at(1000 + i)))
            .collect();
        let da_eliminare = prune_plan(entries, MAX_FILES);
        assert_eq!(da_eliminare.len(), 2);
        assert!(da_eliminare.contains(&PathBuf::from("f00.mfc")));
        assert!(da_eliminare.contains(&PathBuf::from("f01.mfc")));
    }

    #[test]
    fn piu_recente_per_primo() {
        let v = vec![
            (PathBuf::from("a.mfc"), at(5)),
            (PathBuf::from("b.mfc"), at(9)),
            (PathBuf::from("c.mfc"), at(7)),
        ];
        let ord = newest_first(v);
        assert_eq!(ord[0].0, PathBuf::from("b.mfc"));
        assert_eq!(ord[2].0, PathBuf::from("a.mfc"));
    }

    #[test]
    fn scrittura_e_pulizia_su_disco() {
        let t = TempDir::new("prune");
        let dir = t.0.join("autosave");
        // 12 progetti con date diverse: dopo la pulizia restano i 10 più recenti.
        for i in 0..12u64 {
            let p = write_autosave(&dir, &format!("Brano {i}"), "{}").unwrap();
            let f = fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(at(1000 + i * 10)).unwrap();
        }
        prune(&dir, MAX_FILES);
        let mut rimasti: Vec<String> = list_autosaves(&dir)
            .into_iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        rimasti.sort();
        assert_eq!(rimasti.len(), MAX_FILES);
        assert!(!rimasti.contains(&"Brano 0.mfc".to_string()));
        assert!(!rimasti.contains(&"Brano 1.mfc".to_string()));
        assert!(rimasti.contains(&"Brano 11.mfc".to_string()));
        // Nessun file temporaneo residuo.
        assert!(fs::read_dir(&dir)
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn scarto_dopo_salvataggio() {
        let t = TempDir::new("discard");
        let dir = t.0.join("autosave");
        write_autosave(&dir, "Demo", "{}").unwrap();
        assert_eq!(list_autosaves(&dir).len(), 1);
        let _ = fs::remove_file(dir.join(file_name_for("Demo")));
        assert!(list_autosaves(&dir).is_empty());
    }

    #[test]
    fn lock_rileva_sessione_caduta_e_si_rimuove() {
        let t = TempDir::new("lock");
        let dir = t.0.join("autosave");
        let lock = t.0.join("session.lock");

        // Sessione precedente caduta: lock presente e una copia automatica.
        fs::write(&lock, "1234").unwrap();
        write_autosave(&dir, "Perso", "{}").unwrap();
        let mut a = Autosave::default();
        a.start_in(dir.clone(), lock.clone());
        let rec = a.recovery.as_ref().expect("recupero proposto");
        assert_eq!(rec.len(), 1);
        assert!(lock.exists());

        // Uscita pulita: il lock viene rimosso.
        drop(a);
        assert!(!lock.exists());

        // Avvio senza lock precedente: nessun recupero.
        let mut b = Autosave::default();
        b.start_in(dir, lock.clone());
        assert!(b.recovery.is_none());
        assert!(lock.exists());
    }
}
