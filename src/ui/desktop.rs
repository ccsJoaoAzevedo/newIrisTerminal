//! Pinning the main window to the desktop: it survives Win+D.
//!
//! Pinned, the window is an ordinary one in every other way - other windows
//! cover it, and clicking it brings it to the front - but showing the desktop
//! does not take it away. Win+D (and the "show desktop" corner of the taskbar)
//! does two things, in an order that is not guaranteed: it makes the desktop
//! the foreground window, and it minimizes everything. So both are answered.
//! When the desktop comes to the front the window is put back if it was
//! minimized and raised above the desktop as topmost; when anything else comes
//! to the front it stops being topmost and drops in behind whatever that was,
//! which is where an ordinary window would have been.
//!
//! Done in Win32 directly, from a `SetWinEventHook` callback, rather than by
//! asking egui: eframe skips `update` entirely while the window is minimized,
//! so the app would never get the frame it needed to restore itself. The hook
//! is out-of-context, so Windows delivers it through the main thread's message
//! loop - the one winit is already pumping - and nothing is polled.
//!
//! Everywhere else this is a no-op, and the setting simply does nothing.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowPlacement, IsIconic, SetWindowPos, ShowWindow,
        EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZESTART, HWND_NOTOPMOST, HWND_TOPMOST,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_MAXIMIZE, SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
        WINEVENT_OUTOFCONTEXT, WPF_RESTORETOMAXIMIZED,
    };

    /// The main window. Zero until `install` has been handed it.
    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    static PINNED: AtomicBool = AtomicBool::new(false);
    /// Whether this module made the window topmost, and so owes it the undoing.
    static RAISED: AtomicBool = AtomicBool::new(false);
    /// When the desktop last came to the front, in milliseconds since `EPOCH`.
    static DESKTOP_AT: AtomicU64 = AtomicU64::new(0);
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    /// The window is kept above all others by the always-on-top setting, so
    /// `lower` must not take the topmost flag away from it.
    static ON_TOP: AtomicBool = AtomicBool::new(false);
    /// Set by the app's own minimize button, which must still minimize.
    static OWN_MINIMIZE: AtomicBool = AtomicBool::new(false);

    /// How long after the desktop came forward a minimize is still taken to be
    /// Win+D's. Generous, because the shell does the two a variable time apart,
    /// and short enough that a minimize a moment later is the user's.
    const WIN_D_WINDOW_MS: u64 = 1_500;

    fn now_ms() -> u64 {
        EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
    }

    fn window() -> Option<HWND> {
        let raw = WINDOW.load(Ordering::Relaxed);
        (raw != 0).then_some(raw as HWND)
    }

    pub fn install(hwnd: isize) {
        if hwnd == 0 || WINDOW.swap(hwnd, Ordering::Relaxed) != 0 {
            return;
        }
        // SAFETY: the callback is a plain `extern "system"` function with the
        // signature WINEVENTPROC asks for, and it lives for the program. The
        // hooks are never unhooked: they live exactly as long as the window.
        unsafe {
            // Every process: the desktop is Explorer's window, not ours.
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                std::ptr::null_mut(),
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            // Only this one: nobody else's minimizing is any of our business.
            SetWinEventHook(
                EVENT_SYSTEM_MINIMIZESTART,
                EVENT_SYSTEM_MINIMIZESTART,
                std::ptr::null_mut(),
                Some(on_event),
                GetCurrentProcessId(),
                0,
                WINEVENT_OUTOFCONTEXT,
            );
        }
    }

    pub fn set_pinned(pinned: bool) {
        PINNED.store(pinned, Ordering::Relaxed);
        if !pinned {
            lower(None);
        }
    }

    pub fn minimizing_on_purpose() {
        OWN_MINIMIZE.store(true, Ordering::Relaxed);
    }

    pub fn set_on_top(on_top: bool) {
        ON_TOP.store(on_top, Ordering::Relaxed);
    }

    /// Whether `hwnd` is the desktop: Explorer's `Progman`, or the `WorkerW`
    /// it puts in front of it while the wallpaper is animating or Win+D is on.
    fn is_desktop(hwnd: HWND) -> bool {
        let mut name = [0u16; 32];
        // SAFETY: the buffer and its length agree.
        let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        let name = String::from_utf16_lossy(&name[..len.max(0) as usize]);
        name == "Progman" || name == "WorkerW"
    }

    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        let Some(ours) = window() else {
            return;
        };
        if !PINNED.load(Ordering::Relaxed) {
            return;
        }
        match event {
            EVENT_SYSTEM_FOREGROUND if is_desktop(hwnd) => {
                DESKTOP_AT.store(now_ms(), Ordering::Relaxed);
                show_over_desktop(ours);
            }
            EVENT_SYSTEM_FOREGROUND if hwnd == ours => {
                // Clicked, which is the user bringing it forward: it is the
                // foreground window now, and needs no help staying up.
                lower(None);
            }
            EVENT_SYSTEM_FOREGROUND => lower(Some(hwnd)),
            EVENT_SYSTEM_MINIMIZESTART if hwnd == ours => {
                if OWN_MINIMIZE.swap(false, Ordering::Relaxed) {
                    return;
                }
                let at = DESKTOP_AT.load(Ordering::Relaxed);
                if at != 0 && now_ms().saturating_sub(at) < WIN_D_WINDOW_MS {
                    show_over_desktop(ours);
                }
            }
            _ => {}
        }
    }

    /// Puts the window back if Win+D took it, and above the desktop.
    fn show_over_desktop(ours: HWND) {
        // SAFETY: `ours` is the live main window; every call here tolerates a
        // window that has just gone, by failing.
        unsafe {
            if IsIconic(ours) != 0 {
                let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
                placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                let maximized = GetWindowPlacement(ours, &mut placement) != 0
                    && placement.flags & WPF_RESTORETOMAXIMIZED != 0;
                // There is no "maximize without activating"; a maximized
                // window comes back activated, which costs nothing here.
                ShowWindow(
                    ours,
                    if maximized {
                        SW_MAXIMIZE
                    } else {
                        SW_SHOWNOACTIVATE
                    },
                );
            }
            SetWindowPos(
                ours,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        RAISED.store(true, Ordering::Relaxed);
    }

    /// Undoes `show_over_desktop`, and puts the window behind `front` - the
    /// window that just came forward - rather than leaving it over it.
    fn lower(front: Option<HWND>) {
        let Some(ours) = window() else {
            return;
        };
        // Always-on-top owns the flag then; dropping it here is what would
        // make a pinned, always-on-top window fall behind the next click.
        if !RAISED.swap(false, Ordering::Relaxed) || ON_TOP.load(Ordering::Relaxed) {
            return;
        }
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // SAFETY: as above.
        unsafe {
            SetWindowPos(ours, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
            if let Some(front) = front {
                SetWindowPos(ours, front, 0, 0, 0, 0, flags);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn install(_hwnd: isize) {}
    pub fn set_pinned(_pinned: bool) {}
    pub fn minimizing_on_purpose() {}
    pub fn set_on_top(_on_top: bool) {}
}

/// Starts watching for the desktop, for the main window `cc` was made for.
pub fn install(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let hwnd = match cc.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    };
    imp::install(hwnd);
}

/// Turns pinning on or off. Off takes back anything pinning did to the window.
pub fn set_pinned(pinned: bool) {
    imp::set_pinned(pinned)
}

/// Keeps the main window above every other window, or stops.
///
/// Through egui, which knows how on every platform; this module is only told
/// so that pinning to the desktop does not undo it on Windows, where both are
/// the same topmost flag.
pub fn set_always_on_top(ctx: &egui::Context, on_top: bool) {
    imp::set_on_top(on_top);
    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if on_top {
        egui::WindowLevel::AlwaysOnTop
    } else {
        egui::WindowLevel::Normal
    }));
}

/// Says the minimize about to happen is the user's own, so a pinned window
/// still minimizes from its own button.
pub fn minimizing_on_purpose() {
    imp::minimizing_on_purpose()
}
