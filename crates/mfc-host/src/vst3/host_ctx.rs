//! Contesto host passato al plugin: `IHostApplication`, `IComponentHandler`, `IPlugFrame`,
//! `IRunLoop` (Linux), `IPlugInterfaceSupport`, più `IMessage` / `IAttributeList` per la
//! comunicazione componente ↔ controller.
//!
//! Tutto lo stato è dietro mutex/atomici/code lock-free: il plugin può chiamare da
//! qualunque thread.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use parking_lot::Mutex;
use vst3::Steinberg::Linux::*;
use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface};

use crate::EditorSize;

/// Nome dell'host comunicato ai plugin.
pub(crate) const HOST_NAME: &str = "Mixfonocraft";
/// Capacità delle code dei parametri.
pub(crate) const PARAM_QUEUE_CAP: usize = 4096;

/// Coda limitata (MPMC, senza allocazioni dopo la creazione) che, se piena, scarta il
/// cambio più vecchio.
pub(crate) struct ParamQueue {
    tx: Sender<(u32, f64)>,
    rx: Receiver<(u32, f64)>,
}

impl ParamQueue {
    fn new() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(PARAM_QUEUE_CAP);
        Self { tx, rx }
    }

    pub(crate) fn push(&self, id: u32, value: f64) {
        if let Err(TrySendError::Full(v)) = self.tx.try_send((id, value)) {
            let _ = self.rx.try_recv();
            let _ = self.tx.try_send(v);
        }
    }

    pub(crate) fn pop(&self) -> Option<(u32, f64)> {
        self.rx.try_recv().ok()
    }

    pub(crate) fn clear(&self) {
        while self.rx.try_recv().is_ok() {}
    }
}

/// Handler di file descriptor registrato.
type FdHandler = (ComPtr<IEventHandler>, i32);

/// Handler registrati tramite `IRunLoop`.
#[derive(Default)]
struct RunLoop {
    timers: Vec<(ComPtr<ITimerHandler>, Duration, Instant)>,
    fds: Vec<FdHandler>,
}

/// Stato condiviso tra istanza e contesto host.
pub(crate) struct Shared {
    /// Cambi diretti al processore (applicati al blocco audio successivo).
    pub(crate) to_proc: ParamQueue,
    /// Cambi diretti al controller (applicati sul thread UI).
    pub(crate) to_ctrl: ParamQueue,
    /// Ultima richiesta di ridimensionamento dell'editor.
    pub(crate) resize: Mutex<Option<EditorSize>>,
    /// Flag di `restartComponent` non ancora gestiti.
    pub(crate) restart: AtomicI32,
    /// Modifiche fatte dall'utente nella GUI del plugin (`performEdit`), lette dal thread UI.
    pub(crate) gui_edits: crate::ParamChangeLog,
    run_loop: Mutex<RunLoop>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            to_proc: ParamQueue::new(),
            to_ctrl: ParamQueue::new(),
            gui_edits: crate::ParamChangeLog::default(),
            resize: Mutex::new(None),
            restart: AtomicI32::new(0),
            run_loop: Mutex::new(RunLoop::default()),
        }
    }

    /// Esegue timer scaduti e handler dei file descriptor pronti (thread UI).
    pub(crate) fn run_loop_idle(&self) {
        let now = Instant::now();
        let (timers, fds): (Vec<ComPtr<ITimerHandler>>, Vec<FdHandler>) = {
            let mut rl = self.run_loop.lock();
            let mut due = Vec::new();
            for (h, every, last) in rl.timers.iter_mut() {
                if now.duration_since(*last) >= *every {
                    *last = now;
                    due.push(h.clone());
                }
            }
            (due, rl.fds.clone())
        };
        // Callback fuori dal lock: il plugin può (de)registrare handler al loro interno.
        for t in timers {
            // SAFETY: handler valido (riferimento posseduto).
            unsafe { t.onTimer() };
        }
        for (h, fd) in fds {
            if fd_ready(fd) {
                // SAFETY: come sopra.
                unsafe { h.onFDIsSet(fd) };
            }
        }
    }

    /// Rilascia gli handler registrati (prima di scaricare il modulo).
    pub(crate) fn clear_run_loop(&self) {
        let old = std::mem::take(&mut *self.run_loop.lock());
        drop(old);
    }
}

#[cfg(target_os = "linux")]
fn fd_ready(fd: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: un solo pollfd valido, timeout zero.
    let r = unsafe { libc::poll(&mut p, 1, 0) };
    r > 0 && (p.revents & libc::POLLIN) != 0
}

#[cfg(not(target_os = "linux"))]
fn fd_ready(_fd: i32) -> bool {
    false
}

/// Copia una stringa in un buffer UTF-16 terminato da zero.
pub(crate) fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let max = dst.len().saturating_sub(1);
    let mut n = 0;
    for (d, s) in dst.iter_mut().take(max).zip(src.encode_utf16()) {
        *d = s as TChar;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

/// Contesto host di un'istanza.
pub(crate) struct HostContext {
    pub(crate) shared: Arc<Shared>,
}

impl HostContext {
    pub(crate) fn new(shared: Arc<Shared>) -> ComWrapper<HostContext> {
        ComWrapper::new(Self { shared })
    }
}

impl Class for HostContext {
    type Interfaces = (
        IHostApplication,
        IComponentHandler,
        IPlugFrame,
        IRunLoop,
        IPlugInterfaceSupport,
    );
}

/// Crea un oggetto host e restituisce l'interfaccia richiesta in `obj`.
unsafe fn create_host_object<C>(obj: C, iid: *const TUID, out: *mut *mut c_void) -> tresult
where
    C: Class + 'static,
    C::Interfaces: vst3::com_scrape_types::MakeHeader<C, ComWrapper<C>>,
{
    let Some(unknown) = ComWrapper::new(obj).to_com_ptr::<FUnknown>() else {
        return kNoInterface;
    };
    let ptr = unknown.as_ptr();
    ((*(*ptr).vtbl).queryInterface)(ptr, iid, out)
}

fn tuid_eq(a: *const TUID, iid: &TUID) -> bool {
    // SAFETY: il chiamante passa un TUID valido (controllato non nullo).
    !a.is_null() && unsafe { &*a } == iid
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        copy_wstring(HOST_NAME, &mut *name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if obj.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        *obj = std::ptr::null_mut();
        if tuid_eq(cid, &IMessage_iid) {
            create_host_object(HostMessage::new(), iid, obj)
        } else if tuid_eq(cid, &IAttributeList_iid) {
            create_host_object(AttributeList::default(), iid, obj)
        } else {
            kNoInterface
        }
    }
}

impl IComponentHandlerTrait for HostContext {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        // Modifica dalla GUI del plugin: il controller la conosce già, la giriamo al
        // processore e la registriamo per la GUI dell'host (automazioni, progetto).
        let v = value.clamp(0.0, 1.0);
        self.shared.to_proc.push(id, v);
        self.shared.gui_edits.push(id, v);
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.shared.restart.fetch_or(flags, Ordering::AcqRel);
        kResultOk
    }
}

impl IPlugFrameTrait for HostContext {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        let r = *new_size;
        *self.shared.resize.lock() = Some(EditorSize {
            width: (r.right - r.left).max(1) as u32,
            height: (r.bottom - r.top).max(1) as u32,
        });
        // La finestra verrà ridimensionata dalla GUI; confermiamo subito al plugin.
        if let Some(v) = ComRef::from_raw(view) {
            v.onSize(new_size);
        }
        kResultTrue
    }
}

impl IRunLoopTrait for HostContext {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> tresult {
        let Some(h) = ComRef::from_raw(handler) else {
            return kInvalidArgument;
        };
        self.shared.run_loop.lock().fds.push((h.to_com_ptr(), fd));
        kResultTrue
    }
    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> tresult {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let removed: Vec<_> = {
            let mut rl = self.shared.run_loop.lock();
            let (gone, keep) = std::mem::take(&mut rl.fds)
                .into_iter()
                .partition(|(h, _)| h.as_ptr() == handler);
            rl.fds = keep;
            gone
        };
        drop(removed);
        kResultTrue
    }
    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, ms: TimerInterval) -> tresult {
        let Some(h) = ComRef::from_raw(handler) else {
            return kInvalidArgument;
        };
        let every = Duration::from_millis(ms.max(1));
        self.shared
            .run_loop
            .lock()
            .timers
            .push((h.to_com_ptr(), every, Instant::now()));
        kResultTrue
    }
    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let removed: Vec<_> = {
            let mut rl = self.shared.run_loop.lock();
            let (gone, keep) = std::mem::take(&mut rl.timers)
                .into_iter()
                .partition(|(h, _, _)| h.as_ptr() == handler);
            rl.timers = keep;
            gone
        };
        drop(removed);
        kResultTrue
    }
}

impl IPlugInterfaceSupportTrait for HostContext {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        if iid.is_null() {
            return kInvalidArgument;
        }
        let supported = [
            IComponent_iid,
            IAudioProcessor_iid,
            IEditController_iid,
            IConnectionPoint_iid,
        ];
        if supported.iter().any(|s| tuid_eq(iid, s)) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

// ---------------------------------------------------------------------------------------
// Messaggi
// ---------------------------------------------------------------------------------------

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

/// `IAttributeList` dell'host.
#[derive(Default)]
pub(crate) struct AttributeList {
    map: Mutex<HashMap<CString, Attr>>,
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

unsafe fn attr_key(id: AttrID) -> Option<CString> {
    if id.is_null() {
        None
    } else {
        Some(CStr::from_ptr(id).to_owned())
    }
}

impl AttributeList {
    unsafe fn set(&self, id: AttrID, v: Attr) -> tresult {
        match attr_key(id) {
            Some(k) => {
                self.map.lock().insert(k, v);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        self.set(id, Attr::Int(value))
    }
    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.map.lock().get(&k) {
            Some(Attr::Int(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        self.set(id, Attr::Float(value))
    }
    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.map.lock().get(&k) {
            Some(Attr::Float(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut v = Vec::new();
        let mut p = string;
        while *p != 0 && v.len() < 1 << 20 {
            v.push(*p);
            p = p.add(1);
        }
        v.push(0);
        self.set(id, Attr::Str(v))
    }
    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        let map = self.map.lock();
        let Some(Attr::Str(s)) = map.get(&k) else {
            return kResultFalse;
        };
        let cap = size_bytes as usize / std::mem::size_of::<TChar>();
        if string.is_null() || cap == 0 {
            return kInvalidArgument;
        }
        let n = s.len().min(cap);
        std::ptr::copy_nonoverlapping(s.as_ptr(), string, n);
        *string.add(n - 1) = 0;
        kResultOk
    }
    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_bytes: uint32) -> tresult {
        if data.is_null() && size_bytes > 0 {
            return kInvalidArgument;
        }
        let v = if size_bytes == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data as *const u8, size_bytes as usize).to_vec()
        };
        self.set(id, Attr::Bin(v))
    }
    unsafe fn getBinary(&self, id: AttrID, data: *mut *const c_void, size_bytes: *mut uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        if data.is_null() || size_bytes.is_null() {
            return kInvalidArgument;
        }
        match self.map.lock().get(&k) {
            Some(Attr::Bin(b)) => {
                // Il buffer resta valido finché l'attributo non viene sovrascritto.
                *data = b.as_ptr() as *const c_void;
                *size_bytes = b.len() as uint32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

/// `IMessage` dell'host.
pub(crate) struct HostMessage {
    id: Mutex<Option<CString>>,
    attrs: ComWrapper<AttributeList>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            id: Mutex::new(None),
            attrs: ComWrapper::new(AttributeList::default()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        // Il puntatore resta valido finché il messaggio vive e l'id non cambia.
        self.id.lock().as_ref().map_or(std::ptr::null(), |c| c.as_ptr())
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        *self.id.lock() = if id.is_null() {
            None
        } else {
            Some(CStr::from_ptr(id as *const c_char).to_owned())
        };
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attrs
            .as_com_ref::<IAttributeList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

/// Puntatore (senza addRef) a un'interfaccia del contesto host.
pub(crate) fn host_ptr<I: Interface>(h: &ComWrapper<HostContext>) -> *mut I {
    h.as_com_ref::<I>().map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_creates_messages_and_records_edits() {
        let shared = Arc::new(Shared::new());
        let host = HostContext::new(shared.clone());
        unsafe {
            let app: ComRef<IHostApplication> = host.as_com_ref().unwrap();
            let mut name: String128 = [0; 128];
            assert_eq!(app.getName(&mut name), kResultOk);
            let len = name.iter().position(|c| *c == 0).unwrap();
            assert_eq!(String::from_utf16_lossy(&name[..len]), HOST_NAME);

            let mut cid = IMessage_iid;
            let mut iid = IMessage_iid;
            let mut obj = std::ptr::null_mut();
            assert_eq!(app.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
            let msg = ComPtr::from_raw(obj as *mut IMessage).unwrap();
            msg.setMessageID(c"hello".as_ptr());
            assert_eq!(CStr::from_ptr(msg.getMessageID()), c"hello");
            let attrs = ComRef::from_raw(msg.getAttributes()).unwrap();
            assert_eq!(attrs.setInt(c"n".as_ptr(), 42), kResultOk);
            let mut v = 0;
            assert_eq!(attrs.getInt(c"n".as_ptr(), &mut v), kResultOk);
            assert_eq!(v, 42);
            let data = [1u8, 2, 3];
            attrs.setBinary(c"b".as_ptr(), data.as_ptr().cast(), 3);
            let (mut p, mut n) = (std::ptr::null(), 0);
            assert_eq!(attrs.getBinary(c"b".as_ptr(), &mut p, &mut n), kResultOk);
            assert_eq!(std::slice::from_raw_parts(p as *const u8, n as usize), &data);

            let mut cid = [0; 16];
            assert_ne!(app.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
            assert!(obj.is_null());

            let handler: ComRef<IComponentHandler> = host.as_com_ref().unwrap();
            handler.performEdit(7, 0.25);
            handler.restartComponent(RestartFlags_::kLatencyChanged);
            assert_eq!(shared.to_proc.pop(), Some((7, 0.25)));
            handler.performEdit(7, 0.5);
            handler.performEdit(3, 2.0);
            assert_eq!(shared.gui_edits.drain(), vec![(7, 0.5), (3, 1.0)]);
            shared.to_proc.clear();
            assert_eq!(shared.restart.load(Ordering::Relaxed), RestartFlags_::kLatencyChanged);

            let frame: ComRef<IPlugFrame> = host.as_com_ref().unwrap();
            let mut r = ViewRect {
                left: 0,
                top: 0,
                right: 300,
                bottom: 200,
            };
            assert_eq!(frame.resizeView(std::ptr::null_mut(), &mut r), kResultTrue);
            assert_eq!(
                shared.resize.lock().take(),
                Some(EditorSize {
                    width: 300,
                    height: 200
                })
            );
        }
    }

    #[test]
    fn param_queue_drops_oldest_when_full() {
        let q = super::ParamQueue::new();
        for i in 0..(PARAM_QUEUE_CAP as u32 + 5) {
            q.push(i, 0.0);
        }
        assert_eq!(q.pop(), Some((5, 0.0)));
        q.clear();
        assert_eq!(q.pop(), None);
    }
}
