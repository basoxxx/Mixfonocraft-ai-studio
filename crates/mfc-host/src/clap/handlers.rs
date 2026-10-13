//! Callback dell'host CLAP.
//!
//! Tutte le richieste del plugin diventano flag atomici in [`Flags`], letti e gestiti dal
//! thread UI in `ClapInstance::idle`. Nessun callback blocca o alloca (tranne il log).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl};
use clack_extensions::latency::{HostLatency, HostLatencyImpl};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{HostNotePorts, HostNotePortsImpl, NoteDialects, NotePortRescanFlags};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_host::prelude::*;

/// Richieste pendenti del plugin.
#[derive(Default)]
pub(crate) struct Flags {
    pub restart: AtomicBool,
    pub callback: AtomicBool,
    pub flush: AtomicBool,
    pub rescan_values: AtomicBool,
    pub latency_changed: AtomicBool,
    pub dirty: AtomicBool,
    /// Editor chiuso dal plugin; `gui_destroyed` = va chiamato `destroy`.
    pub gui_closed: AtomicBool,
    pub gui_destroyed: AtomicBool,
    /// Ridimensionamento richiesto (larghezza/altezza impacchettate, 0 = nessuno).
    pub resize: AtomicU64,
}

impl Flags {
    pub fn take(flag: &AtomicBool) -> bool {
        flag.swap(false, Ordering::AcqRel)
    }

    pub fn take_resize(&self) -> Option<GuiSize> {
        match self.resize.swap(0, Ordering::AcqRel) {
            0 => None,
            v => Some(GuiSize::unpack_from_u64(v)),
        }
    }
}

/// Tipo che lega i gestori dell'host.
pub(crate) struct MfcHost;

impl HostHandlers for MfcHost {
    type Shared<'a> = SharedHandlerImpl;
    type MainThread<'a> = MainThreadImpl;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostGui>()
            .register::<HostAudioPorts>()
            .register::<HostNotePorts>()
            .register::<HostLatency>();
    }
}

/// Gestore `[thread-safe]`.
pub(crate) struct SharedHandlerImpl {
    pub flags: Arc<Flags>,
}

impl<'a> SharedHandler<'a> for SharedHandlerImpl {
    fn request_restart(&self) {
        self.flags.restart.store(true, Ordering::Release);
    }
    fn request_process(&self) {
        // Il motore elabora sempre: niente da fare.
    }
    fn request_callback(&self) {
        self.flags.callback.store(true, Ordering::Release);
    }
}

impl HostLogImpl for SharedHandlerImpl {
    fn log(&self, severity: LogSeverity, message: &str) {
        if matches!(severity, LogSeverity::Warning | LogSeverity::Error | LogSeverity::Fatal) {
            eprintln!("[clap {severity}] {message}");
        }
    }
}

impl HostParamsImplShared for SharedHandlerImpl {
    fn request_flush(&self) {
        self.flags.flush.store(true, Ordering::Release);
    }
}

impl HostGuiImpl for SharedHandlerImpl {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), clack_host::host::HostError> {
        let packed = new_size.pack_to_u64();
        if packed == 0 {
            return Err(clack_host::host::HostError::Message("dimensione nulla"));
        }
        self.flags.resize.store(packed, Ordering::Release);
        Ok(())
    }

    fn request_show(&self) -> Result<(), clack_host::host::HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), clack_host::host::HostError> {
        Ok(())
    }

    fn closed(&self, was_destroyed: bool) {
        self.flags.gui_destroyed.fetch_or(was_destroyed, Ordering::AcqRel);
        self.flags.gui_closed.store(true, Ordering::Release);
    }
}

/// Gestore `[main-thread]`: solo flag atomici, quindi robusto anche a plugin che lo
/// chiamano dal thread sbagliato.
pub(crate) struct MainThreadImpl {
    pub flags: Arc<Flags>,
}

impl<'a> MainThreadHandler<'a> for MainThreadImpl {}

impl HostParamsImplMainThread for MainThreadImpl {
    fn rescan(&self, flags: ParamRescanFlags) {
        if flags.intersects(ParamRescanFlags::VALUES | ParamRescanFlags::ALL | ParamRescanFlags::INFO) {
            self.flags.rescan_values.store(true, Ordering::Release);
        }
    }
    fn clear(&self, _param_id: clack_host::utils::ClapId, _flags: ParamClearFlags) {}
}

impl HostStateImpl for MainThreadImpl {
    fn mark_dirty(&self) {
        self.flags.dirty.store(true, Ordering::Release);
    }
}

impl HostAudioPortsImpl for MainThreadImpl {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        true
    }
    fn rescan(&self, _flags: AudioPortRescanFlags) {
        // Le porte vengono rilette alla riattivazione.
        self.flags.restart.store(true, Ordering::Release);
    }
}

impl HostNotePortsImpl for MainThreadImpl {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }
    fn rescan(&self, _flags: NotePortRescanFlags) {
        self.flags.restart.store(true, Ordering::Release);
    }
}

impl HostLatencyImpl for MainThreadImpl {
    fn changed(&self) {
        self.flags.latency_changed.store(true, Ordering::Release);
    }
}
