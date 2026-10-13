//! Interfaccia degli aggiornamenti: notifica in basso a destra, avanzamento del
//! download, note di rilascio, conferma con modifiche non salvate, menu "Aiuto".
//!
//! `Updater::default()` è inerte (nessuna rete, nessun file di configurazione): lo usano
//! i test headless. L'app reale usa [`Updater::load`], che arma il controllo all'avvio.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, RichText};

use crate::state::Studio;
use crate::theme::pal;
use crate::update::{self, InstallOutcome, Platform, Progress, Release, UpdateSettings};

/// Fase corrente dell'aggiornamento.
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// Controllo in corso; `manual` = richiesto dall'utente (mostra anche "nessun aggiornamento").
    Checking {
        rx: Receiver<Result<Release, String>>,
        manual: bool,
    },
    /// Nuova versione disponibile.
    Available(Release),
    /// Download del pacchetto in corso.
    Downloading { release: Release, progress: Arc<Progress> },
    /// Pacchetto scaricato, pronto da installare.
    Ready { release: Release, file: PathBuf },
}

/// Stato dell'aggiornamento automatico.
#[derive(Default)]
pub struct Updater {
    settings: UpdateSettings,
    /// Salva le impostazioni su disco (falso nei test).
    persist: bool,
    /// Controllo automatico da eseguire al primo frame.
    pending_auto: bool,
    phase: Phase,
    /// Notifica nascosta con "Più tardi".
    dismissed: bool,
    show_notes: bool,
    confirm_unsaved: bool,
    about_open: bool,
    /// Messaggio da riportare nella barra di stato.
    message: Option<String>,
}

impl Updater {
    /// Carica le impostazioni e programma il controllo all'avvio (max uno ogni 24 ore).
    pub fn load() -> Self {
        let settings = UpdateSettings::load();
        let pending_auto = settings.check_on_startup && settings.auto_check_due(update::now_secs());
        Self {
            settings,
            persist: true,
            pending_auto,
            ..Default::default()
        }
    }

    /// `true` durante controllo o download.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking { .. } | Phase::Downloading { .. })
    }

    /// Avvia un controllo in background.
    pub fn check(&mut self, ctx: &egui::Context, manual: bool) {
        if self.busy() {
            return;
        }
        self.settings.last_check = update::now_secs();
        self.save_settings();
        self.phase = Phase::Checking {
            rx: update::spawn_check(Some(ctx.clone())),
            manual,
        };
    }

    fn save_settings(&self) {
        if self.persist {
            self.settings.save();
        }
    }

    fn release(&self) -> Option<&Release> {
        match &self.phase {
            Phase::Available(r) => Some(r),
            Phase::Downloading { release, .. } | Phase::Ready { release, .. } => Some(release),
            _ => None,
        }
    }

    /// Aggiorna la fase in base ai risultati dei thread in background.
    fn poll(&mut self, ctx: &egui::Context) {
        self.phase = match std::mem::take(&mut self.phase) {
            Phase::Checking { rx, manual } => match rx.try_recv() {
                Ok(Ok(release)) if update::is_newer(update::CURRENT_VERSION, &release.version) => {
                    self.dismissed = false;
                    Phase::Available(release)
                }
                Ok(Ok(_)) => {
                    if manual {
                        self.message = Some(format!(
                            "Mixfonocraft è aggiornato (versione {})",
                            update::CURRENT_VERSION
                        ));
                    }
                    Phase::Idle
                }
                Ok(Err(e)) => {
                    if manual {
                        self.message = Some(format!("Aggiornamenti: {e}"));
                    }
                    Phase::Idle
                }
                Err(TryRecvError::Disconnected) => Phase::Idle,
                Err(TryRecvError::Empty) => Phase::Checking { rx, manual },
            },
            Phase::Downloading { release, progress } => match progress.take_result() {
                Some(Ok(file)) => Phase::Ready { release, file },
                Some(Err(e)) => {
                    self.message = Some(if progress.cancel.load(Ordering::Relaxed) {
                        "Download dell'aggiornamento annullato".into()
                    } else {
                        format!("Download dell'aggiornamento non riuscito: {e}")
                    });
                    Phase::Available(release)
                }
                None => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    Phase::Downloading { release, progress }
                }
            },
            other => other,
        };
    }

    /// "Aggiorna ora": scarica il pacchetto della piattaforma o apre la pagina della release.
    fn start_download(&mut self, ctx: &egui::Context) {
        let Phase::Available(release) = std::mem::take(&mut self.phase) else {
            return;
        };
        match update::select_asset(&release.assets, Platform::current()) {
            Some(asset) => {
                let progress = update::spawn_download(asset.clone(), Some(ctx.clone()));
                self.phase = Phase::Downloading { release, progress };
            }
            None => {
                self.message = Some(match update::install(None, &release.page_url) {
                    Ok(InstallOutcome::Manual(m)) => m,
                    Ok(InstallOutcome::Quit) => String::new(),
                    Err(e) => format!("Aggiornamento: {e}"),
                });
                self.dismissed = true;
                self.phase = Phase::Available(release);
            }
        }
    }

    /// Avvia l'installazione; se l'installer è partito chiude l'app.
    fn install(&mut self, ctx: &egui::Context) {
        let Phase::Ready { release, file } = &self.phase else {
            return;
        };
        match update::install(Some(file), &release.page_url) {
            Ok(InstallOutcome::Quit) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Ok(InstallOutcome::Manual(m)) => self.message = Some(m),
            Err(e) => self.message = Some(format!("Aggiornamento: {e}")),
        }
    }
}

/// Disegna notifiche e finestre degli aggiornamenti.
pub fn show(ctx: &egui::Context, studio: &mut Studio, up: &mut Updater) {
    if std::mem::take(&mut up.pending_auto) {
        up.check(ctx, false);
    }
    up.poll(ctx);

    if !up.dismissed && up.release().is_some() {
        toast(ctx, studio, up);
    }
    notes_window(ctx, up);
    unsaved_window(ctx, studio, up);
    about_window(ctx, up);

    if let Some(m) = up.message.take().filter(|m| !m.is_empty()) {
        studio.status = m;
    }
}

fn toast(ctx: &egui::Context, studio: &Studio, up: &mut Updater) {
    egui::Area::new(egui::Id::new("mfc-update-toast"))
        .order(egui::Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(300.0);
                let Some(release) = up.release() else { return };
                ui.label(
                    RichText::new(format!("Disponibile Mixfonocraft {}", release.version))
                        .strong()
                        .color(pal::ACCENT()),
                );
                ui.label(
                    RichText::new(format!("Versione in uso: {}", update::CURRENT_VERSION))
                        .small()
                        .color(pal::TEXT_DIM()),
                );
                ui.add_space(6.0);
                match &up.phase {
                    Phase::Available(_) => {
                        ui.horizontal(|ui| {
                            if ui.button("Novità").clicked() {
                                up.show_notes = true;
                            }
                            if ui.button(RichText::new("Aggiorna ora").color(pal::ACCENT())).clicked() {
                                up.start_download(ui.ctx());
                            }
                            if ui.button("Più tardi").clicked() {
                                up.dismissed = true;
                            }
                        });
                    }
                    Phase::Downloading { progress, .. } => {
                        let bar = match progress.fraction() {
                            Some(f) => egui::ProgressBar::new(f).show_percentage(),
                            None => egui::ProgressBar::new(0.0).animate(true).text(format!(
                                "{:.1} MB",
                                progress.bytes.load(Ordering::Relaxed) as f64 / 1_048_576.0
                            )),
                        };
                        ui.add(bar);
                        if ui.button("Annulla").clicked() {
                            progress.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    Phase::Ready { .. } => {
                        ui.label("Download completato.");
                        ui.horizontal(|ui| {
                            if ui
                                .button(RichText::new("Installa e riavvia").color(pal::ACCENT()))
                                .clicked()
                            {
                                if studio.dirty {
                                    up.confirm_unsaved = true;
                                } else {
                                    up.install(ui.ctx());
                                }
                            }
                            if ui.button("Più tardi").clicked() {
                                up.dismissed = true;
                            }
                        });
                    }
                    _ => {}
                }
            });
        });
}

fn notes_window(ctx: &egui::Context, up: &mut Updater) {
    if !up.show_notes {
        return;
    }
    let Some(release) = up.release() else {
        up.show_notes = false;
        return;
    };
    let title = format!("Novità di Mixfonocraft {}", release.version);
    let notes = if release.notes.trim().is_empty() {
        "Nessuna nota di rilascio.".to_string()
    } else {
        release.notes.clone()
    };
    let page = release.page_url.clone();
    let mut open = true;
    let mut err = None;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .default_size([460.0, 360.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.label(notes);
            });
            ui.separator();
            if ui.link("Apri la pagina della release").clicked() {
                err = update::open_external(&page).err();
            }
        });
    up.show_notes = open;
    if let Some(e) = err {
        up.message = Some(e);
    }
}

fn unsaved_window(ctx: &egui::Context, studio: &mut Studio, up: &mut Updater) {
    if !up.confirm_unsaved {
        return;
    }
    let mut choice = None;
    egui::Window::new("Modifiche non salvate")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("Il progetto ha modifiche non salvate. Salvarle prima di aggiornare?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Salva e aggiorna").clicked() {
                    choice = Some(0);
                }
                if ui.button("Aggiorna senza salvare").clicked() {
                    choice = Some(1);
                }
                if ui.button("Annulla").clicked() {
                    choice = Some(2);
                }
            });
        });
    match choice {
        Some(0) => {
            let path = studio.path.clone().or_else(|| {
                rfd::FileDialog::new()
                    .set_title("Salva progetto")
                    .add_filter("Progetto Mixfonocraft", &["mfc"])
                    .set_file_name("progetto.mfc")
                    .save_file()
            });
            let Some(path) = path else { return };
            match studio.save_as(&path) {
                Ok(()) => {
                    up.confirm_unsaved = false;
                    up.install(ctx);
                }
                Err(e) => up.message = Some(format!("Errore: {e}")),
            }
        }
        Some(1) => {
            up.confirm_unsaved = false;
            up.install(ctx);
        }
        Some(_) => up.confirm_unsaved = false,
        None => {}
    }
}

fn about_window(ctx: &egui::Context, up: &mut Updater) {
    let mut open = up.about_open;
    egui::Window::new("Informazioni")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(RichText::new("Mixfonocraft AI Studio").strong().color(pal::TEXT()));
            ui.label(format!("Versione {}", update::CURRENT_VERSION));
            ui.hyperlink_to("Pagina del progetto", format!("https://github.com/{}", update::REPO));
        });
    up.about_open = open;
}

/// Contenuto del menu "Aiuto".
pub fn help_menu(ui: &mut egui::Ui, up: &mut Updater) {
    ui.set_min_width(240.0);
    let label = if up.busy() {
        "Controllo in corso…"
    } else {
        "Controlla aggiornamenti…"
    };
    if ui.add_enabled(!up.busy(), egui::Button::new(label)).clicked() {
        up.check(ui.ctx(), true);
        ui.close();
    }
    if ui
        .checkbox(&mut up.settings.check_on_startup, "Controlla all'avvio")
        .changed()
    {
        up.save_settings();
    }
    ui.separator();
    if ui.button("Pagina delle release").clicked() {
        up.message = update::open_external(update::RELEASES_PAGE).err();
        ui.close();
    }
    if ui.button("Informazioni su Mixfonocraft…").clicked() {
        up.about_open = true;
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inert() {
        let up = Updater::default();
        assert!(!up.pending_auto);
        assert!(!up.persist);
        assert!(!up.busy());
        assert!(up.release().is_none());
    }

    #[test]
    fn check_result_newer_becomes_available() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut rel = update::parse_release(include_str!("../../tests/fixtures/github_release_latest.json")).unwrap();
        rel.version = semver::Version::new(999, 0, 0);
        tx.send(Ok(rel)).unwrap();
        let mut up = Updater {
            phase: Phase::Checking { rx, manual: true },
            ..Default::default()
        };
        up.poll(&egui::Context::default());
        assert!(matches!(up.phase, Phase::Available(_)));
    }

    #[test]
    fn check_result_older_reports_up_to_date() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut rel = update::parse_release(include_str!("../../tests/fixtures/github_release_latest.json")).unwrap();
        rel.version = semver::Version::new(0, 0, 1);
        tx.send(Ok(rel)).unwrap();
        let mut up = Updater {
            phase: Phase::Checking { rx, manual: true },
            ..Default::default()
        };
        up.poll(&egui::Context::default());
        assert!(matches!(up.phase, Phase::Idle));
        assert!(up.message.as_deref().unwrap().contains("aggiornato"));
    }

    #[test]
    fn auto_check_error_is_silent() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("rete assente".into())).unwrap();
        let mut up = Updater {
            phase: Phase::Checking { rx, manual: false },
            ..Default::default()
        };
        up.poll(&egui::Context::default());
        assert!(matches!(up.phase, Phase::Idle));
        assert!(up.message.is_none());
    }
}
