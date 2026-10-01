//! Closing to the notification area: the window goes, the sessions stay.
//!
//! Done in Win32 directly, like [`crate::ui::desktop`], and for the same
//! reason: eframe does not call `update` for a window that is not shown, so
//! the app could never answer the click that is meant to bring it back. The
//! icon's messages go to a message-only window of our own, created on the main
//! thread, so winit's message loop delivers them and nothing is polled.
//!
//! The sessions keep reading while the window is hidden - their channels are
//! unbounded - so IRIS is never blocked; what arrived is drawn when the window
//! comes back.
//!
//! Everywhere else this is a no-op, and closing simply closes.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetCursorPos,
        LoadIconW, PostMessageW, RegisterClassW, SetForegroundWindow, ShowWindow, TrackPopupMenu,
        HICON, HWND_MESSAGE, IDI_APPLICATION, MF_STRING, SW_HIDE, SW_RESTORE, SW_SHOW,
        TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
    };

    /// The main window. Zero until `install` has been handed it.
    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    /// The message-only window the icon reports to.
    static SINK: AtomicIsize = AtomicIsize::new(0);
    static SHOWN: AtomicBool = AtomicBool::new(false);
    /// Set by the menu's Exit, so the close it raises is not turned back into
    /// a hide.
    static QUITTING: AtomicBool = AtomicBool::new(false);

    const CALLBACK: u32 = WM_APP + 1;
    const ICON_ID: u32 = 1;
    const CMD_OPEN: usize = 1;
    const CMD_EXIT: usize = 2;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn handle(atom: &AtomicIsize) -> Option<HWND> {
        let raw = atom.load(Ordering::Relaxed);
        (raw != 0).then_some(raw as HWND)
    }

    pub fn install(hwnd: isize) {
        if hwnd == 0 || WINDOW.swap(hwnd, Ordering::Relaxed) != 0 {
            return;
        }
        let class = wide("newIrisTerminalTray");
        // SAFETY: the class name outlives the calls that read it, and the
        // window procedure has the signature WNDPROC asks for.
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(on_message),
                hInstance: GetModuleHandleW(std::ptr::null()),
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let sink = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                wc.hInstance,
                std::ptr::null(),
            );
            SINK.store(sink as isize, Ordering::Relaxed);
        }
    }

    fn icon_data(sink: HWND) -> NOTIFYICONDATAW {
        // SAFETY: an all-zero NOTIFYICONDATAW is a valid empty one.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = sink;
        data.uID = ICON_ID;
        data
    }

    fn app_icon() -> HICON {
        // SAFETY: resource 1 is the icon `build.rs` embeds; the stock icon is
        // the fallback for a build without it.
        unsafe {
            let icon = LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as _);
            if icon.is_null() {
                LoadIconW(std::ptr::null_mut(), IDI_APPLICATION)
            } else {
                icon
            }
        }
    }

    pub fn hide(tip: &str) -> bool {
        let (Some(window), Some(sink)) = (handle(&WINDOW), handle(&SINK)) else {
            return false;
        };
        let mut data = icon_data(sink);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = app_icon();
        let tip: Vec<u16> = tip.encode_utf16().take(data.szTip.len() - 1).collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
        // SAFETY: `data` is fully initialised and names our own sink window.
        unsafe {
            if !SHOWN.swap(true, Ordering::Relaxed) && Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                SHOWN.store(false, Ordering::Relaxed);
                // Without an icon there would be no way back to the window.
                return false;
            }
            ShowWindow(window, SW_HIDE);
        }
        true
    }

    fn restore() {
        if let Some(sink) = handle(&SINK) {
            if SHOWN.swap(false, Ordering::Relaxed) {
                // SAFETY: deletes only the icon this module added.
                unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(sink)) };
            }
        }
        if let Some(window) = handle(&WINDOW) {
            // SAFETY: plain calls on the main window's handle.
            unsafe {
                ShowWindow(window, SW_SHOW);
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
            }
        }
    }

    pub fn quitting() -> bool {
        QUITTING.load(Ordering::Relaxed)
    }

    fn menu(sink: HWND) {
        let open = wide(crate::i18n::tr("Open"));
        let exit = wide(crate::i18n::tr("Exit"));
        // SAFETY: the strings outlive the menu, which is destroyed here.
        let chosen = unsafe {
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, CMD_OPEN, open.as_ptr());
            AppendMenuW(menu, MF_STRING, CMD_EXIT, exit.as_ptr());
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&mut at);
            // Without this the menu does not go away when clicked outside.
            SetForegroundWindow(sink);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                at.x,
                at.y,
                0,
                sink,
                std::ptr::null(),
            );
            DestroyMenu(menu);
            chosen as usize
        };
        match chosen {
            CMD_OPEN => restore(),
            CMD_EXIT => {
                QUITTING.store(true, Ordering::Relaxed);
                // Shown first, so a close confirmation has somewhere to appear.
                restore();
                if let Some(window) = handle(&WINDOW) {
                    // SAFETY: posts an ordinary close to our own window.
                    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
                }
            }
            _ => {}
        }
    }

    unsafe extern "system" fn on_message(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == CALLBACK {
            match lparam as u32 {
                WM_LBUTTONUP => restore(),
                WM_RBUTTONUP => menu(hwnd),
                _ => {}
            }
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn install(_hwnd: isize) {}
    pub fn hide(_tip: &str) -> bool {
        false
    }
    pub fn quitting() -> bool {
        false
    }
}

/// Makes the tray available for the main window `cc` was made for.
pub fn install(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let hwnd = match cc.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    };
    imp::install(hwnd);
}

/// Hides the window behind an icon in the notification area. False when that
/// could not be done, in which case the caller should close as it would have.
pub fn hide(tip: &str) -> bool {
    imp::hide(tip)
}

/// Whether the tray menu's Exit asked for the close now under way, which must
/// close rather than hide again.
pub fn quitting() -> bool {
    imp::quitting()
}
