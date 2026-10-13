//! Finestra "Impostazioni audio": dispositivo di uscita, frequenza di campionamento e buffer.
//! La configurazione è una bozza: solo "Applica" riavvia il backend audio.

use egui::RichText;

use crate::state::{AudioConfig, Studio};
use crate::theme::pal;
use crate::views::widgets::section_label;

/// Dimensioni di buffer proposte (in campioni).
#[cfg(feature = "realtime")]
const BUFFER_SIZES: [u32; 5] = [128, 256, 512, 1024, 2048];

/// Stato della finestra impostazioni audio.
#[derive(Default)]
pub struct AudioSettings {
    /// Finestra aperta.
    pub open: bool,
    /// Bozza della configurazione, non ancora applicata.
    draft: AudioConfig,
    /// Dispositivi di uscita disponibili (aggiornati all'apertura e su richiesta).
    #[cfg(feature = "realtime")]
    devices: Vec<String>,
    /// Frequenze selezionabili per il dispositivo della bozza.
    #[cfg(feature = "realtime")]
    rates: Vec<u32>,
}

impl AudioSettings {
    /// Apre la finestra partendo dalla configurazione attiva.
    pub fn request_open(&mut self, studio: &Studio) {
        self.open = true;
        self.draft = studio.audio_config.clone();
        self.refresh_devices();
    }

    /// Rilegge dispositivi e frequenze disponibili.
    fn refresh_devices(&mut self) {
        #[cfg(feature = "realtime")]
        {
            use mfc_engine::backend::device::{output_devices, supported_sample_rates};
            self.devices = output_devices();
            self.rates = supported_sample_rates(self.draft.device.as_deref());
        }
    }
}

/// Disegna la finestra (se aperta).
pub fn show(ctx: &egui::Context, studio: &mut Studio, st: &mut AudioSettings) {
    if !st.open {
        return;
    }
    let mut open = st.open;
    egui::Window::new("Impostazioni audio")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            body(ui, studio, st);
            ui.separator();
            recording_and_plugins(ui, studio);
        });
    st.open = open;
}

/// Ingresso per la registrazione audio e modalità di esecuzione dei plugin esterni.
fn recording_and_plugins(ui: &mut egui::Ui, studio: &mut Studio) {
    section_label(ui, "Registrazione audio");
    let devices = crate::audio_rec::AudioRec::devices();
    ui.add_enabled_ui(!studio.audio_rec.is_active(), |ui| {
        ui.checkbox(
            &mut studio.audio_rec.enabled,
            "Registra l'ingresso audio con il pulsante Record",
        );
        let current = studio
            .audio_rec
            .device
            .clone()
            .unwrap_or_else(|| "Ingresso predefinito".into());
        egui::ComboBox::from_id_salt("audio_input")
            .width(300.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(studio.audio_rec.device.is_none(), "Ingresso predefinito")
                    .clicked()
                {
                    studio.audio_rec.device = None;
                }
                for d in &devices {
                    let sel = studio.audio_rec.device.as_deref() == Some(d.as_str());
                    if ui.selectable_label(sel, d).clicked() {
                        studio.audio_rec.device = Some(d.clone());
                    }
                }
            });
    });
    if devices.is_empty() {
        ui.label(
            RichText::new("Nessun ingresso audio trovato")
                .size(11.0)
                .color(pal::TEXT_DIM),
        );
    }
    ui.add_space(6.0);

    section_label(ui, "Plugin VST3 / CLAP");
    let mut sandbox = studio.host.sandbox_enabled();
    if ui
        .checkbox(&mut sandbox, "Esegui i plugin in un processo separato")
        .on_hover_text(
            "Se un plugin va in crash l'app resta aperta. Aggiunge un blocco di latenza; vale per i plugin \
             caricati da ora in poi.",
        )
        .changed()
    {
        studio.status = match studio.host.set_sandbox(sandbox) {
            Ok(()) if sandbox => "Plugin in un processo separato (dai prossimi caricamenti)".into(),
            Ok(()) => "Plugin nello stesso processo dell'app".into(),
            Err(e) => format!("Modalità separata non disponibile: {e}"),
        };
    }
}

#[cfg(not(feature = "realtime"))]
fn body(ui: &mut egui::Ui, _studio: &mut Studio, _st: &mut AudioSettings) {
    ui.label(RichText::new("Audio non disponibile in questa build").color(pal::TEXT_DIM));
}

#[cfg(feature = "realtime")]
fn body(ui: &mut egui::Ui, studio: &mut Studio, st: &mut AudioSettings) {
    ui.set_width(340.0);

    // Stato attuale.
    section_label(ui, "Stato");
    let sr = studio.engine.sample_rate() as u32;
    let (text, color) = if studio.audible {
        (format!("Uscita attiva · {sr} Hz"), pal::GREEN)
    } else {
        ("Nessuna scheda audio: uscita silenziosa".to_string(), pal::YELLOW)
    };
    ui.label(RichText::new(text).color(color));
    ui.add_space(6.0);

    // Dispositivo.
    section_label(ui, "Dispositivo di uscita");
    let before = st.draft.device.clone();
    let dev_text = st
        .draft
        .device
        .clone()
        .unwrap_or_else(|| "Predefinito di sistema".into());
    egui::ComboBox::from_id_salt("audio_device")
        .width(320.0)
        .selected_text(dev_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut st.draft.device, None, "Predefinito di sistema");
            for d in &st.devices {
                ui.selectable_value(&mut st.draft.device, Some(d.clone()), d);
            }
        });
    if st.draft.device != before {
        // Frequenze diverse per dispositivo: ricalcola e scarta quella non supportata.
        st.rates = mfc_engine::backend::device::supported_sample_rates(st.draft.device.as_deref());
        if st.draft.sample_rate.is_some_and(|r| !st.rates.contains(&r)) {
            st.draft.sample_rate = None;
        }
    }
    ui.add_space(4.0);

    // Frequenza di campionamento.
    section_label(ui, "Frequenza di campionamento");
    let rate_text = st
        .draft
        .sample_rate
        .map_or("Predefinita".to_string(), |r| format!("{r} Hz"));
    egui::ComboBox::from_id_salt("audio_rate")
        .width(320.0)
        .selected_text(rate_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut st.draft.sample_rate, None, "Predefinita");
            for r in &st.rates {
                ui.selectable_value(&mut st.draft.sample_rate, Some(*r), format!("{r} Hz"));
            }
        });
    ui.add_space(4.0);

    // Buffer.
    section_label(ui, "Dimensione buffer");
    let buf_text = st
        .draft
        .buffer_size
        .map_or("Default".to_string(), |n| format!("{n} campioni"));
    egui::ComboBox::from_id_salt("audio_buffer")
        .width(320.0)
        .selected_text(buf_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut st.draft.buffer_size, None, "Default");
            for n in BUFFER_SIZES {
                ui.selectable_value(&mut st.draft.buffer_size, Some(n), format!("{n} campioni"));
            }
        });
    if let Some(n) = st.draft.buffer_size {
        let rate = st.draft.sample_rate.unwrap_or(sr).max(1);
        let ms = n as f32 * 1000.0 / rate as f32;
        ui.label(
            RichText::new(format!("Latenza di buffer ≈ {ms:.1} ms a {rate} Hz"))
                .size(11.0)
                .color(pal::TEXT_FAINT),
        );
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Aggiorna dispositivi").clicked() {
            st.refresh_devices();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(RichText::new("Applica").strong()).clicked() {
                studio.restart_audio(st.draft.clone());
            }
        });
    });
    if !studio.status.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(&studio.status).size(11.0).color(pal::TEXT_DIM));
    }
}
