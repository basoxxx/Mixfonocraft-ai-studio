//! Oggetti COM lato host usati nel processing e nello stato: lista eventi, code dei
//! parametri (preallocate, nessuna allocazione nel thread audio) e stream in memoria.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;

use parking_lot::Mutex;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

/// Capacità della lista eventi per blocco.
pub(crate) const MAX_EVENTS: usize = 2048;
/// Parametri distinti per blocco.
pub(crate) const MAX_PARAM_QUEUES: usize = 512;
/// Punti per coda di parametro.
pub(crate) const MAX_POINTS: usize = 64;

// ---------------------------------------------------------------------------------------
// Lista eventi
// ---------------------------------------------------------------------------------------

/// `IEventList` a capacità fissa. Usata solo dal thread che possiede lo stato audio
/// (protetto dal mutex del processore), quindi `UnsafeCell` senza sincronizzazione.
pub(crate) struct EventList {
    events: UnsafeCell<Vec<Event>>,
}

impl EventList {
    pub(crate) fn new() -> Self {
        Self {
            events: UnsafeCell::new(Vec::with_capacity(MAX_EVENTS)),
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn list(&self) -> &mut Vec<Event> {
        // SAFETY: accesso esclusivo garantito dal mutex dello stato audio; il plugin
        // chiama i metodi solo dentro `process`, sullo stesso thread.
        unsafe { &mut *self.events.get() }
    }

    pub(crate) fn clear(&self) {
        self.list().clear();
    }

    /// Aggiunge un evento se c'è spazio (mai rialloca).
    pub(crate) fn push(&self, e: Event) {
        let l = self.list();
        if l.len() < l.capacity() {
            l.push(e);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.list().len()
    }

    /// Ordina per offset (insertion sort stabile, senza allocazioni).
    pub(crate) fn sort_by_offset(&self) {
        let l = self.list();
        for i in 1..l.len() {
            let mut j = i;
            while j > 0 && l[j - 1].sampleOffset > l[j].sampleOffset {
                l.swap(j - 1, j);
                j -= 1;
            }
        }
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.list().len() as int32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match (
            usize::try_from(index).ok().and_then(|i| self.list().get(i)),
            e.is_null(),
        ) {
            (Some(ev), false) => {
                *e = *ev;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        let l = self.list();
        if l.len() >= l.capacity() {
            return kOutOfMemory;
        }
        l.push(*e);
        kResultOk
    }
}

/// Evento di nota pronto per la lista.
pub(crate) fn note_event(on: bool, offset: i32, pitch: i16, velocity: f32, note_id: i32) -> Event {
    // SAFETY: `Event` è POD (union di struct numeriche/puntatori): tutti zeri è valido.
    let mut e: Event = unsafe { std::mem::zeroed() };
    e.busIndex = 0;
    e.sampleOffset = offset;
    e.flags = Event_::EventFlags_::kIsLive as u16;
    if on {
        e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
        e.__field0.noteOn = NoteOnEvent {
            channel: 0,
            pitch,
            tuning: 0.0,
            velocity,
            length: 0,
            noteId: note_id,
        };
    } else {
        e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
        e.__field0.noteOff = NoteOffEvent {
            channel: 0,
            pitch,
            velocity,
            noteId: note_id,
            tuning: 0.0,
        };
    }
    e
}

// ---------------------------------------------------------------------------------------
// Code dei parametri
// ---------------------------------------------------------------------------------------

/// `IParamValueQueue` con punti preallocati.
pub(crate) struct ParamQueue {
    id: Cell<ParamID>,
    points: UnsafeCell<Vec<(i32, f64)>>,
}

impl ParamQueue {
    fn new() -> Self {
        Self {
            id: Cell::new(0),
            points: UnsafeCell::new(Vec::with_capacity(MAX_POINTS)),
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn points(&self) -> &mut Vec<(i32, f64)> {
        // SAFETY: come per `EventList`: accesso esclusivo sotto il mutex dello stato audio.
        unsafe { &mut *self.points.get() }
    }

    fn reset(&self, id: ParamID) {
        self.id.set(id);
        self.points().clear();
    }

    /// Inserisce un punto ordinato per offset (stesso offset → sostituisce).
    fn add(&self, offset: i32, value: f64) -> Option<usize> {
        let p = self.points();
        let pos = p.partition_point(|q| q.0 < offset);
        if let Some(q) = p.get_mut(pos) {
            if q.0 == offset {
                q.1 = value;
                return Some(pos);
            }
        }
        if p.len() >= p.capacity() {
            return None;
        }
        p.insert(pos, (offset, value));
        Some(pos)
    }

    fn last(&self) -> Option<f64> {
        self.points().last().map(|p| p.1)
    }
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.points().len() as int32
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        let Some(p) = usize::try_from(index).ok().and_then(|i| self.points().get(i)) else {
            return kInvalidArgument;
        };
        if !offset.is_null() {
            *offset = p.0;
        }
        if !value.is_null() {
            *value = p.1;
        }
        kResultOk
    }
    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        match self.add(offset, value) {
            Some(i) => {
                if !index.is_null() {
                    *index = i as int32;
                }
                kResultOk
            }
            None => kOutOfMemory,
        }
    }
}

/// `IParameterChanges` con code preallocate.
pub(crate) struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    used: Cell<usize>,
}

impl ParamChanges {
    pub(crate) fn new() -> Self {
        Self {
            queues: (0..MAX_PARAM_QUEUES)
                .map(|_| ComWrapper::new(ParamQueue::new()))
                .collect(),
            used: Cell::new(0),
        }
    }

    pub(crate) fn clear(&self) {
        self.used.set(0);
    }

    fn queue_ptr(&self, i: usize) -> *mut IParamValueQueue {
        self.queues
            .get(i)
            .and_then(|q| q.as_com_ref::<IParamValueQueue>())
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }

    /// Indice della coda per `id`, creandola se serve.
    fn queue_index(&self, id: ParamID) -> Option<usize> {
        let used = self.used.get();
        if let Some(i) = self.queues[..used].iter().position(|q| q.id.get() == id) {
            return Some(i);
        }
        let q = self.queues.get(used)?;
        q.reset(id);
        self.used.set(used + 1);
        Some(used)
    }

    /// Imposta il valore (punto a offset 0). `false` se le code sono piene.
    pub(crate) fn set(&self, id: ParamID, value: f64) -> bool {
        match self.queue_index(id) {
            Some(i) => self.queues[i].add(0, value).is_some(),
            None => false,
        }
    }

    /// Ultimo valore di ogni parametro cambiato.
    pub(crate) fn for_each_last(&self, mut f: impl FnMut(ParamID, f64)) {
        for q in &self.queues[..self.used.get()] {
            if let Some(v) = q.last() {
                f(q.id.get(), v);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.used.get()
    }
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        match usize::try_from(index) {
            Ok(i) if i < self.used.get() => self.queue_ptr(i),
            _ => std::ptr::null_mut(),
        }
    }
    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        match self.queue_index(*id) {
            Some(i) => {
                if !index.is_null() {
                    *index = i as int32;
                }
                self.queue_ptr(i)
            }
            None => std::ptr::null_mut(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Stream in memoria
// ---------------------------------------------------------------------------------------

struct MemInner {
    data: Vec<u8>,
    pos: usize,
}

/// `IBStream` in memoria (lettura/scrittura/seek/tell).
pub(crate) struct MemStream {
    inner: Mutex<MemInner>,
}

impl MemStream {
    pub(crate) fn new(data: Vec<u8>) -> ComWrapper<MemStream> {
        ComWrapper::new(Self {
            inner: Mutex::new(MemInner { data, pos: 0 }),
        })
    }

    pub(crate) fn rewind(&self) {
        self.inner.lock().pos = 0;
    }

    pub(crate) fn take(&self) -> Vec<u8> {
        let mut g = self.inner.lock();
        g.pos = 0;
        std::mem::take(&mut g.data)
    }
}

/// Puntatore `IBStream` (senza addRef) da passare al plugin.
pub(crate) fn stream_ptr(s: &ComWrapper<MemStream>) -> *mut IBStream {
    s.as_com_ref::<IBStream>().map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        if num_bytes < 0 || (buffer.is_null() && num_bytes > 0) {
            return kInvalidArgument;
        }
        let mut g = self.inner.lock();
        let pos = g.pos.min(g.data.len());
        let n = (num_bytes as usize).min(g.data.len() - pos);
        if n > 0 {
            std::ptr::copy_nonoverlapping(g.data.as_ptr().add(pos), buffer as *mut u8, n);
        }
        g.pos = pos + n;
        if !num_read.is_null() {
            *num_read = n as int32;
        }
        // Come lo stream dell'SDK: fine dati con 0 byte letti è comunque kResultOk.
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_written: *mut int32) -> tresult {
        if num_bytes < 0 || (buffer.is_null() && num_bytes > 0) {
            return kInvalidArgument;
        }
        let n = num_bytes as usize;
        let mut g = self.inner.lock();
        let pos = g.pos;
        // Limite prudente (1 GiB) contro seek fuori misura seguiti da scritture.
        let Some(end) = pos.checked_add(n).filter(|e| *e <= 1 << 30) else {
            return kOutOfMemory;
        };
        if g.data.len() < end {
            g.data.resize(end, 0);
        }
        if n > 0 {
            std::ptr::copy_nonoverlapping(buffer as *const u8, g.data.as_mut_ptr().add(pos), n);
        }
        g.pos = pos + n;
        if !num_written.is_null() {
            *num_written = n as int32;
        }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.inner.lock();
        let base = match mode as IBStream_::IStreamSeekMode {
            IBStream_::IStreamSeekMode_::kIBSeekSet => 0i64,
            IBStream_::IStreamSeekMode_::kIBSeekCur => g.pos as i64,
            IBStream_::IStreamSeekMode_::kIBSeekEnd => g.data.len() as i64,
            _ => return kInvalidArgument,
        };
        let Some(new) = base.checked_add(pos).filter(|p| *p >= 0) else {
            return kInvalidArgument;
        };
        g.pos = new as usize;
        if !result.is_null() {
            *result = new;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = self.inner.lock().pos as int64;
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::ComRef;

    #[test]
    fn param_changes_are_preallocated_and_merged() {
        let pc = ComWrapper::new(ParamChanges::new());
        assert!(pc.set(3, 0.1));
        assert!(pc.set(3, 0.7));
        assert!(pc.set(5, 0.2));
        assert_eq!(pc.len(), 2);
        let mut seen = vec![];
        pc.for_each_last(|id, v| seen.push((id, v)));
        assert_eq!(seen, vec![(3, 0.7), (5, 0.2)]);
        unsafe {
            let r: ComRef<IParameterChanges> = pc.as_com_ref().unwrap();
            assert_eq!(r.getParameterCount(), 2);
            let mut idx = -1;
            let q = ComRef::from_raw(r.addParameterData(&9, &mut idx)).unwrap();
            assert_eq!(idx, 2);
            let mut i = 0;
            assert_eq!(q.addPoint(10, 0.5, &mut i), kResultOk);
            assert_eq!(q.addPoint(2, 0.4, &mut i), kResultOk);
            assert_eq!(i, 0);
            let (mut o, mut v) = (0, 0.0);
            assert_eq!(q.getPoint(1, &mut o, &mut v), kResultOk);
            assert_eq!((o, v), (10, 0.5));
            assert!(r.getParameterData(7).is_null());
        }
        pc.clear();
        assert_eq!(pc.len(), 0);
        for id in 0..MAX_PARAM_QUEUES as u32 {
            assert!(pc.set(id, 0.0));
        }
        assert!(!pc.set(10_000, 0.0));
    }

    #[test]
    fn mem_stream_read_write_seek() {
        let s = MemStream::new(vec![]);
        unsafe {
            let r: ComRef<IBStream> = s.as_com_ref().unwrap();
            let mut buf = *b"abcdef";
            let mut n = 0;
            assert_eq!(r.write(buf.as_mut_ptr().cast(), 6, &mut n), kResultOk);
            assert_eq!(n, 6);
            let mut p = 0;
            assert_eq!(
                r.seek(2, IBStream_::IStreamSeekMode_::kIBSeekSet as i32, &mut p),
                kResultOk
            );
            let mut out = [0u8; 10];
            assert_eq!(r.read(out.as_mut_ptr().cast(), 10, &mut n), kResultOk);
            assert_eq!(&out[..n as usize], b"cdef");
            r.tell(&mut p);
            assert_eq!(p, 6);
            assert_eq!(
                r.seek(-1, IBStream_::IStreamSeekMode_::kIBSeekEnd as i32, &mut p),
                kResultOk
            );
            assert_eq!(p, 5);
            assert_ne!(
                r.seek(-10, IBStream_::IStreamSeekMode_::kIBSeekCur as i32, &mut p),
                kResultOk
            );
        }
        assert_eq!(s.take(), b"abcdef");
    }

    #[test]
    fn event_list_is_bounded() {
        let l = ComWrapper::new(EventList::new());
        for i in 0..MAX_EVENTS + 10 {
            l.push(note_event(true, i as i32, 60, 1.0, 0));
        }
        assert_eq!(l.len(), MAX_EVENTS);
        unsafe {
            let r: vst3::ComRef<IEventList> = l.as_com_ref().unwrap();
            let mut e: Event = std::mem::zeroed();
            assert_eq!(r.getEvent(5, &mut e), kResultOk);
            assert_eq!(e.sampleOffset, 5);
            assert_eq!(e.__field0.noteOn.pitch, 60);
            assert_ne!(r.getEvent(-1, &mut e), kResultOk);
        }
    }
}
