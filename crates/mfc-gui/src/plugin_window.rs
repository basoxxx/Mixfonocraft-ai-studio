//! Finestre native che ospitano l'editor originale dei plugin VST3/CLAP.
//!
//! - macOS: `NSWindow` (AppKit) — l'editor viene agganciato alla sua `contentView`.
//! - Windows: finestra Win32 di primo livello — l'editor diventa figlio dell'`HWND`.
//!   I messaggi sono smistati dal loop di winit (stesso thread).
//! - Altri sistemi: non supportato, la GUI usa l'editor generico dei parametri.
//!
//! Tutte le funzioni vanno chiamate dal thread UI (su macOS è il main thread).

use std::sync::Arc;

use mfc_host::{EditorSize, ExternalInstance};

/// Una finestra con l'editor nativo di un plugin.
pub struct PluginWindow {
    instance: Arc<dyn ExternalInstance>,
    native: platform::Native,
}

impl PluginWindow {
    /// `true` se la piattaforma corrente supporta gli editor nativi.
    pub const SUPPORTED: bool = platform::SUPPORTED;

    /// Crea la finestra e vi apre l'editor del plugin.
    pub fn open(instance: Arc<dyn ExternalInstance>, title: &str) -> Result<Self, String> {
        if !instance.has_editor() {
            return Err("il plugin non ha un'interfaccia propria".into());
        }
        let native = platform::Native::create(title)?;
        match instance.open_editor(native.parent()) {
            Ok(size) => {
                native.resize(size);
                native.show();
                Ok(Self { instance, native })
            }
            Err(e) => {
                native.destroy();
                Err(e.to_string())
            }
        }
    }

    /// Da chiamare a ogni frame: idle del plugin e ridimensionamenti richiesti.
    /// Restituisce `false` quando l'utente ha chiuso la finestra.
    pub fn poll(&mut self) -> bool {
        if !self.native.is_open() {
            return false;
        }
        self.instance.idle();
        if let Some(size) = self.instance.take_editor_resize() {
            self.native.resize(size);
        }
        true
    }

    /// Porta la finestra in primo piano.
    pub fn focus(&self) {
        self.native.show();
    }
}

impl Drop for PluginWindow {
    fn drop(&mut self) {
        self.instance.close_editor();
        self.native.destroy();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::EditorSize;
    use mfc_host::ParentWindow;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    pub const SUPPORTED: bool = true;

    pub struct Native {
        window: Retained<NSWindow>,
    }

    impl Native {
        pub fn create(title: &str) -> Result<Self, String> {
            let mtm = MainThreadMarker::new().ok_or("gli editor vanno aperti dal main thread")?;
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
            Ok(Self { window })
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
    use super::EditorSize;
    use mfc_host::ParentWindow;
    use std::sync::Once;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub const SUPPORTED: bool = true;

    const CLASS: &[u16] = &[
        b'M' as u16,
        b'f' as u16,
        b'c' as u16,
        b'P' as u16,
        b'l' as u16,
        b'u' as u16,
        b'g' as u16,
        b'i' as u16,
        b'n' as u16,
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

    pub struct Native {
        hwnd: HWND,
    }

    impl Native {
        pub fn create(title: &str) -> Result<Self, String> {
            register();
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
    use super::EditorSize;
    use mfc_host::ParentWindow;

    pub const SUPPORTED: bool = false;

    pub struct Native;

    impl Native {
        pub fn create(_title: &str) -> Result<Self, String> {
            Err("editor nativi non supportati su questa piattaforma: usa l'editor generico".into())
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
