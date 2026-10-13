//! Finestre native dell'helper `mfc-plugin-host` per l'editor del plugin (modalità sandbox).
//!
//! Stessa struttura di `mfc-gui/src/plugin_window.rs`, ma la finestra appartiene al
//! processo figlio, che fa girare anche il proprio loop degli eventi:
//! - macOS: `NSApplication` (politica "accessory": nessuna icona nel Dock) inizializzata
//!   alla prima apertura; gli eventi vengono smistati da [`pump`] sul main thread.
//! - Windows: finestra Win32 di primo livello e coda dei messaggi del main thread.
//! - Altri sistemi: non supportato (l'host usa l'editor generico dei parametri).
//!
//! Tutte le funzioni vanno chiamate dal main thread dell'helper.

pub use platform::{active, pump, EditorWindow, SUPPORTED};

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::{EditorSize, ParentWindow};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEventMask, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};

    pub const SUPPORTED: bool = true;

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    /// `true` dopo l'inizializzazione di `NSApplication` (prima finestra aperta).
    pub fn active() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    fn app(mtm: MainThreadMarker) -> Retained<NSApplication> {
        let app = NSApplication::sharedApplication(mtm);
        if !ACTIVE.swap(true, Ordering::Relaxed) {
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
        }
        app
    }

    /// Smista gli eventi AppKit attendendo al massimo `wait`.
    pub fn pump(wait: Duration) {
        let Some(mtm) = MainThreadMarker::new() else {
            std::thread::sleep(wait);
            return;
        };
        if !active() {
            std::thread::sleep(wait);
            return;
        }
        let app = NSApplication::sharedApplication(mtm);
        let until = NSDate::dateWithTimeIntervalSinceNow(wait.as_secs_f64());
        // SAFETY: costante di AppKit sempre inizializzata.
        let mode = unsafe { NSDefaultRunLoopMode };
        let mut first = true;
        loop {
            // Solo il primo giro attende; poi si svuota la coda senza bloccare.
            let date = if first { Some(&*until) } else { None };
            first = false;
            let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, date, mode, true);
            match event {
                Some(e) => app.sendEvent(&e),
                None => break,
            }
        }
        app.updateWindows();
    }

    pub struct EditorWindow {
        window: Retained<NSWindow>,
        mtm: MainThreadMarker,
    }

    impl EditorWindow {
        pub fn create(title: &str) -> Result<Self, String> {
            let mtm = MainThreadMarker::new().ok_or("gli editor vanno aperti dal main thread")?;
            app(mtm);
            let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
            let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(640.0, 400.0));
            // SAFETY: parametri validi, chiamata dal main thread (mtm).
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            // La finestra resta di nostra proprietà (Retained) anche dopo la chiusura.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(title));
            window.center();
            Ok(Self { window, mtm })
        }

        pub fn parent(&self) -> ParentWindow {
            let view = self.window.contentView().expect("contentView");
            ParentWindow::AppKit(Retained::as_ptr(&view) as *mut std::ffi::c_void)
        }

        pub fn resize(&self, size: EditorSize) {
            self.window
                .setContentSize(NSSize::new(size.width as f64, size.height as f64));
        }

        pub fn show(&self) {
            self.window.makeKeyAndOrderFront(None);
            // `activate` esiste solo da macOS 14: si usa la variante storica.
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm).activateIgnoringOtherApps(true);
        }

        pub fn is_open(&self) -> bool {
            self.window.isVisible()
        }

        pub fn destroy(&self) {
            self.window.orderOut(None);
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use std::time::Duration;

    use crate::{EditorSize, ParentWindow};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub const SUPPORTED: bool = true;

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    /// `true` dopo la creazione della prima finestra.
    pub fn active() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    /// Smista i messaggi Win32 del main thread attendendo al massimo `wait`.
    pub fn pump(wait: Duration) {
        // SAFETY: coda dei messaggi del thread corrente; `msg` è una struttura POD.
        unsafe {
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait.as_millis() as u32, QS_ALLINPUT);
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    const CLASS: &[u16] = &[
        b'M' as u16,
        b'f' as u16,
        b'c' as u16,
        b'S' as u16,
        b'a' as u16,
        b'n' as u16,
        b'd' as u16,
        b'b' as u16,
        b'o' as u16,
        b'x' as u16,
        0,
    ];
    const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_CLOSE {
            // Nascondi soltanto: la distruzione avviene dopo che il plugin ha staccato l'editor.
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    fn register() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: CLASS.as_ptr(),
            };
            RegisterClassW(&wc);
        });
    }

    pub struct EditorWindow {
        hwnd: HWND,
    }

    impl EditorWindow {
        pub fn create(title: &str) -> Result<Self, String> {
            register();
            ACTIVE.store(true, Ordering::Relaxed);
            let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: classe registrata, stringhe terminate da zero.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    CLASS.as_ptr(),
                    title.as_ptr(),
                    STYLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    640,
                    400,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                )
            };
            if hwnd.is_null() {
                return Err("CreateWindowExW fallita".into());
            }
            Ok(Self { hwnd })
        }

        pub fn parent(&self) -> ParentWindow {
            ParentWindow::Win32(self.hwnd as isize)
        }

        pub fn resize(&self, size: EditorSize) {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: size.width as i32,
                bottom: size.height as i32,
            };
            unsafe {
                AdjustWindowRectEx(&mut r, STYLE, 0, 0);
                SetWindowPos(
                    self.hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        pub fn show(&self) {
            unsafe {
                ShowWindow(self.hwnd, SW_SHOW);
                SetForegroundWindow(self.hwnd);
            }
        }

        pub fn is_open(&self) -> bool {
            unsafe { IsWindowVisible(self.hwnd) != 0 }
        }

        pub fn destroy(&self) {
            unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use std::time::Duration;

    use crate::{EditorSize, ParentWindow};

    pub const SUPPORTED: bool = false;

    pub fn active() -> bool {
        false
    }

    pub fn pump(wait: Duration) {
        std::thread::sleep(wait);
    }

    pub struct EditorWindow;

    impl EditorWindow {
        pub fn create(_title: &str) -> Result<Self, String> {
            Err("editor in un processo separato non supportati su questa piattaforma".into())
        }
        pub fn parent(&self) -> ParentWindow {
            ParentWindow::X11(0)
        }
        pub fn resize(&self, _size: EditorSize) {}
        pub fn show(&self) {}
        pub fn is_open(&self) -> bool {
            false
        }
        pub fn destroy(&self) {}
    }
}
