//! Caricamento dei moduli VST3: risoluzione del binario nel bundle, punti d'ingresso
//! (`ModuleEntry` / `InitDll` / `bundleEntry`), fabbrica e classi.
//!
//! I moduli sono condivisi (cache di `Weak` per percorso) e restano caricati finché esiste
//! un'istanza o una scansione che li usa.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use vst3::Steinberg::*;
use vst3::{ComPtr, Interface};

use crate::HostError;

type ExitFn = unsafe extern "system" fn() -> bool;
type GetFactoryFn = unsafe extern "system" fn() -> *mut IPluginFactory;

/// Categoria delle classi audio.
pub(crate) const AUDIO_CLASS: &str = "Audio Module Class";

/// Modulo VST3 caricato.
pub(crate) struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    exit: Option<ExitFn>,
    lib: Option<libloading::Library>,
    #[cfg(target_os = "macos")]
    _bundle: Option<core_foundation::bundle::CFBundle>,
}

// SAFETY: la fabbrica VST3 è thread-safe per specifica; la libreria e il bundle sono
// handle immutabili dopo il caricamento. L'uscita del modulo avviene solo in `Drop`,
// quando nessun altro riferimento esiste.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

/// Classe trovata nella fabbrica.
#[derive(Debug, Clone)]
pub(crate) struct ClassInfo {
    pub cid: TUID,
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sub_categories: String,
}

static CACHE: Mutex<Vec<(PathBuf, Weak<Module>)>> = Mutex::new(Vec::new());

impl Module {
    /// Carica (o riusa) il modulo del bundle/file indicato.
    pub(crate) fn load(path: &Path) -> Result<Arc<Module>, HostError> {
        let mut cache = CACHE.lock();
        cache.retain(|(_, w)| w.strong_count() > 0);
        if let Some(m) = cache.iter().find(|(p, _)| p == path).and_then(|(_, w)| w.upgrade()) {
            return Ok(m);
        }
        let m = Arc::new(Self::open(path)?);
        cache.push((path.to_path_buf(), Arc::downgrade(&m)));
        Ok(m)
    }

    fn open(path: &Path) -> Result<Module, HostError> {
        let err = |reason: String| HostError::Load {
            path: path.to_path_buf(),
            reason,
        };
        let bin = resolve_binary(path).ok_or_else(|| err("binario del plugin non trovato nel bundle".into()))?;
        let mut module = Module {
            factory: None,
            exit: None,
            lib: None,
            #[cfg(target_os = "macos")]
            _bundle: None,
        };
        // SAFETY: caricare un plugin esegue codice di terze parti: è lo scopo dell'host.
        unsafe {
            let (lib, handle) = open_library(&bin).map_err(|e| err(e.to_string()))?;
            let lib = module.lib.insert(lib);
            let (exit, _bundle) = platform_entry(lib, handle, path).map_err(err)?;
            // Da qui in poi, se qualcosa fallisce, `Drop` chiama l'uscita.
            module.exit = exit;
            #[cfg(target_os = "macos")]
            {
                module._bundle = _bundle;
            }
            let lib = module.lib.as_ref().ok_or_else(|| err("libreria".into()))?;
            let get: libloading::Symbol<GetFactoryFn> = lib
                .get(b"GetPluginFactory\0")
                .map_err(|_| err("GetPluginFactory mancante".into()))?;
            let f = get();
            module.factory =
                Some(ComPtr::from_raw(f).ok_or_else(|| err("GetPluginFactory ha restituito null".into()))?);
        }
        Ok(module)
    }

    /// Fabbrica del modulo.
    pub(crate) fn factory(&self) -> Option<&ComPtr<IPluginFactory>> {
        self.factory.as_ref()
    }

    /// Tutte le classi della fabbrica (con le informazioni estese se disponibili).
    pub(crate) fn classes(&self) -> Vec<ClassInfo> {
        let Some(f) = self.factory() else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: chiamate alla fabbrica con strutture azzerate di dimensione corretta.
        unsafe {
            let mut finfo: PFactoryInfo = std::mem::zeroed();
            let factory_vendor = if f.getFactoryInfo(&mut finfo) == kResultOk {
                cstr(&finfo.vendor)
            } else {
                String::new()
            };
            let f2 = f.cast::<IPluginFactory2>();
            let f3 = f.cast::<IPluginFactory3>();
            for i in 0..f.countClasses().max(0) {
                let mut c: PClassInfo = std::mem::zeroed();
                if f.getClassInfo(i, &mut c) != kResultOk {
                    continue;
                }
                let mut info = ClassInfo {
                    cid: c.cid,
                    category: cstr(&c.category),
                    name: cstr(&c.name),
                    vendor: factory_vendor.clone(),
                    version: String::new(),
                    sub_categories: String::new(),
                };
                if let Some(f2) = &f2 {
                    let mut c2: PClassInfo2 = std::mem::zeroed();
                    if f2.getClassInfo2(i, &mut c2) == kResultOk {
                        info.sub_categories = cstr(&c2.subCategories);
                        set_nonempty(&mut info.vendor, cstr(&c2.vendor));
                        info.version = cstr(&c2.version);
                        set_nonempty(&mut info.name, cstr(&c2.name));
                    }
                }
                if let Some(f3) = &f3 {
                    let mut cw: PClassInfoW = std::mem::zeroed();
                    if f3.getClassInfoUnicode(i, &mut cw) == kResultOk {
                        set_nonempty(&mut info.name, wstr(&cw.name));
                        set_nonempty(&mut info.vendor, wstr(&cw.vendor));
                        set_nonempty(&mut info.version, wstr(&cw.version));
                        set_nonempty(&mut info.sub_categories, cstr(&cw.subCategories));
                    }
                }
                out.push(info);
            }
        }
        out
    }

    /// Crea un oggetto della classe `cid` con l'interfaccia `I`.
    pub(crate) fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
        let f = self.factory()?;
        let mut obj: *mut c_void = std::ptr::null_mut();
        // SAFETY: cid/iid sono TUID validi; il risultato è un riferimento posseduto.
        unsafe {
            let r = f.createInstance(cid.as_ptr(), I::IID.as_ptr() as FIDString, &mut obj);
            if r != kResultOk {
                return None;
            }
            ComPtr::from_raw(obj as *mut I)
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // Ordine: rilascio fabbrica → funzione d'uscita → scaricamento della libreria.
        drop(self.factory.take());
        if let Some(exit) = self.exit.take() {
            // SAFETY: simbolo del modulo ancora caricato.
            unsafe {
                exit();
            }
        }
        drop(self.lib.take());
    }
}

fn set_nonempty(dst: &mut String, v: String) {
    if !v.is_empty() {
        *dst = v;
    }
}

/// Stringa C a lunghezza fissa → `String`.
pub(crate) fn cstr(s: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = s.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Stringa UTF-16 a lunghezza fissa → `String`.
pub(crate) fn wstr(s: &[u16]) -> String {
    let len = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len]).trim().to_string()
}

/// Class id come 32 cifre esadecimali maiuscole (ordine dei byte del TUID).
pub(crate) fn cid_to_hex(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

/// Inverso di [`cid_to_hex`].
pub(crate) fn hex_to_cid(s: &str) -> Option<TUID> {
    let s = s.trim();
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()? as std::ffi::c_char;
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------
// Risoluzione del binario
// ---------------------------------------------------------------------------------------

/// Sistema operativo per la risoluzione dei percorsi (separato da `cfg` per i test).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Linux,
    Windows,
    Mac,
}

impl Os {
    pub(crate) fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// Sottocartelle di `Contents` per architettura (in ordine di preferenza) ed estensione.
fn arch_dirs(os: Os, arch: &str) -> (Vec<String>, &'static str) {
    match os {
        Os::Linux => {
            let a = match arch {
                "x86" => "i386",
                "arm" => "armv7l",
                a => a,
            };
            (vec![format!("{a}-linux")], "so")
        }
        Os::Windows => {
            let dirs = match arch {
                "aarch64" => vec!["arm64-win", "arm64ec-win", "arm64x-win"],
                "x86" => vec!["x86-win"],
                _ => vec!["x86_64-win", "arm64ec-win", "arm64x-win"],
            };
            (dirs.into_iter().map(String::from).collect(), "vst3")
        }
        Os::Mac => (vec!["MacOS".to_string()], ""),
    }
}

/// Percorso del binario per il bundle/file `path` (deve esistere).
pub(crate) fn resolve_binary_for(path: &Path, os: Os, arch: &str) -> Option<PathBuf> {
    if path.is_file() {
        // DLL `.vst3` a file singolo (Windows legacy) o binario indicato direttamente.
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let contents = path.join("Contents");
    let (dirs, ext) = arch_dirs(os, arch);
    for d in dirs {
        let dir = contents.join(d);
        let name = if ext.is_empty() {
            stem.clone()
        } else {
            format!("{stem}.{ext}")
        };
        let exact = dir.join(name);
        if exact.is_file() {
            return Some(exact);
        }
        // Nome diverso dal bundle: primo file con l'estensione giusta.
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .filter(|p| ext.is_empty() || p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
            .collect();
        files.sort();
        if let Some(f) = files.into_iter().next() {
            return Some(f);
        }
    }
    None
}

fn resolve_binary(path: &Path) -> Option<PathBuf> {
    resolve_binary_for(path, Os::current(), std::env::consts::ARCH)
}

// ---------------------------------------------------------------------------------------
// Punti d'ingresso per piattaforma
// ---------------------------------------------------------------------------------------

/// Handle nativo passato a `ModuleEntry` (Linux) o non usato.
type RawHandle = *mut c_void;

#[cfg(unix)]
unsafe fn open_library(bin: &Path) -> Result<(libloading::Library, RawHandle), libloading::Error> {
    use libloading::os::unix::{Library, RTLD_LAZY, RTLD_LOCAL};
    let lib = Library::open(Some(bin), RTLD_LAZY | RTLD_LOCAL)?;
    // Recupero dell'handle `dlopen` (serve a `ModuleEntry`) senza chiudere la libreria.
    let handle = lib.into_raw();
    Ok((Library::from_raw(handle).into(), handle))
}

#[cfg(windows)]
unsafe fn open_library(bin: &Path) -> Result<(libloading::Library, RawHandle), libloading::Error> {
    Ok((libloading::Library::new(bin)?, std::ptr::null_mut()))
}

/// Oggetto da tenere vivo insieme al modulo (il `CFBundle` su macOS).
#[cfg(not(target_os = "macos"))]
type BundleHandle = ();
#[cfg(target_os = "macos")]
type BundleHandle = Option<core_foundation::bundle::CFBundle>;

/// Linux: `ModuleEntry(handle)` / `ModuleExit()`.
#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn platform_entry(
    lib: &libloading::Library,
    handle: RawHandle,
    _path: &Path,
) -> Result<(Option<ExitFn>, BundleHandle), String> {
    type EntryFn = unsafe extern "system" fn(*mut c_void) -> bool;
    let entry: Option<EntryFn> = lib.get::<EntryFn>(b"ModuleEntry\0").ok().map(|s| *s);
    let exit: Option<ExitFn> = lib.get::<ExitFn>(b"ModuleExit\0").ok().map(|s| *s);
    if let Some(entry) = entry {
        if !entry(handle) {
            return Err("ModuleEntry ha restituito false".into());
        }
    }
    Ok((exit, ()))
}

/// Windows: `InitDll()` / `ExitDll()` (opzionali).
#[cfg(windows)]
unsafe fn platform_entry(
    lib: &libloading::Library,
    _handle: RawHandle,
    _path: &Path,
) -> Result<(Option<ExitFn>, BundleHandle), String> {
    type EntryFn = unsafe extern "system" fn() -> bool;
    let entry: Option<EntryFn> = lib.get::<EntryFn>(b"InitDll\0").ok().map(|s| *s);
    let exit: Option<ExitFn> = lib.get::<ExitFn>(b"ExitDll\0").ok().map(|s| *s);
    if let Some(entry) = entry {
        if !entry() {
            return Err("InitDll ha restituito false".into());
        }
    }
    Ok((exit, ()))
}

/// macOS: `bundleEntry(CFBundleRef)` / `bundleExit()` (anche le varianti maiuscole).
#[cfg(target_os = "macos")]
unsafe fn platform_entry(
    lib: &libloading::Library,
    _handle: RawHandle,
    path: &Path,
) -> Result<(Option<ExitFn>, BundleHandle), String> {
    use core_foundation::base::TCFType;
    use core_foundation::bundle::CFBundle;
    use core_foundation::url::CFURL;
    type EntryFn = unsafe extern "system" fn(*mut c_void) -> bool;
    let entry: Option<EntryFn> = [&b"bundleEntry\0"[..], b"BundleEntry\0"]
        .iter()
        .find_map(|n| lib.get::<EntryFn>(*n).ok().map(|s| *s));
    let exit: Option<ExitFn> = [&b"bundleExit\0"[..], b"BundleExit\0"]
        .iter()
        .find_map(|n| lib.get::<ExitFn>(*n).ok().map(|s| *s));
    let mut bundle = None;
    if let Some(entry) = entry {
        bundle = CFURL::from_path(path, true).and_then(CFBundle::new);
        let bref = bundle
            .as_ref()
            .map_or(std::ptr::null_mut(), |b| b.as_concrete_TypeRef() as *mut c_void);
        if !entry(bref) {
            return Err("bundleEntry ha restituito false".into());
        }
    }
    Ok((exit, bundle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_roundtrip_as_hex() {
        let cid: TUID = vst3::uid(0x01234567, 0x89ABCDEF, 0xFEDCBA98, 0x76543210);
        let hex = cid_to_hex(&cid);
        assert_eq!(hex.len(), 32);
        assert_eq!(hex_to_cid(&hex), Some(cid));
        assert_eq!(hex_to_cid(&hex.to_lowercase()), Some(cid));
        assert_eq!(hex_to_cid("xyz"), None);
    }

    #[test]
    fn binary_resolution_per_platform() {
        let root = std::env::temp_dir().join(format!("mfc_vst3_resolve_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let b = root.join("Synth.vst3");
        let mk = |p: &str| {
            let f = b.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, b"").unwrap();
            f
        };
        let lin = mk("Contents/x86_64-linux/Synth.so");
        let lin_arm = mk("Contents/aarch64-linux/Other.so");
        let win = mk("Contents/x86_64-win/Synth.vst3");
        let mac = mk("Contents/MacOS/Synth");
        assert_eq!(resolve_binary_for(&b, Os::Linux, "x86_64"), Some(lin));
        assert_eq!(resolve_binary_for(&b, Os::Linux, "aarch64"), Some(lin_arm));
        assert_eq!(resolve_binary_for(&b, Os::Windows, "x86_64"), Some(win));
        assert_eq!(resolve_binary_for(&b, Os::Mac, "aarch64"), Some(mac));
        assert_eq!(resolve_binary_for(&b, Os::Windows, "aarch64"), None);
        // DLL legacy a file singolo.
        let single = root.join("Legacy.vst3");
        std::fs::write(&single, b"").unwrap();
        assert_eq!(resolve_binary_for(&single, Os::Windows, "x86_64"), Some(single.clone()));
        assert!(Module::load(&root.join("Missing.vst3")).is_err());
        assert!(Module::load(&single).is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
