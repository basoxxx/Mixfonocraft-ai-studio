//! Integrazione macOS: apertura dei documenti dal Finder.
//!
//! Il Finder non passa i file come argomenti ma invia l'Apple Event `kAEOpenDocuments`, che
//! AppKit consegna al delegate dell'applicazione come `application:openURLs:`. Il delegate
//! creato da winit non implementa quel metodo (e NSApplication risponderebbe "non può aprire
//! file in formato…"), quindi lo aggiungiamo alla sua classe a runtime, dopo la creazione
//! dell'event loop e prima di `finishLaunching`: così è coperto anche l'avvio a freddo.

use std::ffi::{c_char, CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};

/// Nome della classe del delegate dichiarata da winit 0.30.
const DELEGATE_CLASS: &CStr = c"WinitApplicationDelegate";

/// `- (void)application:(NSApplication *)app openURLs:(NSArray<NSURL *> *)urls`
unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut AnyObject) {
    if urls.is_null() {
        return;
    }
    let urls = unsafe { &*urls };
    let count: usize = unsafe { msg_send![urls, count] };
    let mut paths = Vec::with_capacity(count);
    for i in 0..count {
        let url: *mut AnyObject = unsafe { msg_send![urls, objectAtIndex: i] };
        if url.is_null() {
            continue;
        }
        let url = unsafe { &*url };
        let is_file: bool = unsafe { msg_send![url, isFileURL] };
        if !is_file {
            continue;
        }
        let repr: *const c_char = unsafe { msg_send![url, fileSystemRepresentation] };
        if repr.is_null() {
            continue;
        }
        let bytes = unsafe { CStr::from_ptr(repr) }.to_bytes();
        paths.push(PathBuf::from(OsStr::from_bytes(bytes)));
    }
    crate::open_queue::push(paths);
}

/// Aggiunge `application:openURLs:` al delegate di winit. Va chiamata dopo la creazione
/// dell'event loop (che registra la classe) e prima di eseguirlo.
pub fn install_open_documents_handler() {
    let Some(class) = AnyClass::get(DELEGATE_CLASS) else {
        eprintln!("avviso: delegate di winit non trovato, apertura dei documenti dal Finder non disponibile");
        return;
    };
    let f: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) = open_urls;
    // SAFETY: la firma corrisponde ai tipi dichiarati ("v@:@@": void, self, _cmd, due oggetti).
    let imp: Imp = unsafe { std::mem::transmute::<_, Imp>(f) };
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            sel!(application:openURLs:),
            imp,
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        eprintln!("avviso: application:openURLs: già definito sul delegate");
    }
}
