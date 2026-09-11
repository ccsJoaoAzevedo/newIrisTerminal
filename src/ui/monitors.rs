//! Whether a saved window position is somewhere the user can still reach it.
//!
//! A position is recorded when the window closes and handed back to the window
//! builder on the next launch — see `Settings::restored_window_position`. That
//! is right until the arrangement of monitors changes: a window closed on a
//! second screen is saved at a coordinate that only existed while that screen
//! was attached, and reopening there on a laptop on its own puts the window
//! somewhere with no pixels. There is then no way to get at it — not the
//! title bar, not the Settings window, which is saved the same way and lands in
//! the same nowhere — short of editing `settings.toml` by hand.
//!
//! So the saved position is checked against the monitors that are actually
//! attached *before* the window is built, and a position that no longer lands
//! on one is dropped, which opens the window centred instead.
//!
//! Only Windows has an implementation. Elsewhere every position is accepted,
//! which is the behaviour this module replaced.

/// Points of a window's top edge that have to be on a monitor for the window to
/// count as reachable.
///
/// Not the whole window, and not a single pixel either. A window may legitimately
/// hang off the side of a screen, and one dragged mostly off the bottom is still
/// something the user put there on purpose. What it must keep is enough of its
/// top edge to grab: this app draws its own title bar, and that is the strip the
/// mouse needs.
const GRAB_STRIP: f32 = 120.0;

/// Height of that strip, in points.
const GRAB_HEIGHT: f32 = 24.0;

/// True when a window whose frame starts at `position` and is `size` points
/// across would open somewhere the user can reach it.
///
/// `size` is the window's inner size, which is what the settings file holds. It
/// is only used to decide how much of the top edge to look for, so the frame
/// being a few points larger does not matter.
pub fn reachable(position: [f32; 2], size: Option<[f32; 2]>) -> bool {
    let [x, y] = position;
    if !x.is_finite() || !y.is_finite() {
        return false;
    }
    let width = size.map_or(GRAB_STRIP, |[w, _]| w.clamp(1.0, GRAB_STRIP));
    let strip = Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + GRAB_HEIGHT,
    };
    let screens = work_areas();
    // No monitor could be identified — a platform without an implementation,
    // or an API that failed. Trusting the saved position is the behaviour that
    // was there before, and is the safe way to be wrong: the worst case is the
    // bug this module exists to fix, not a window that refuses to reopen where
    // it was.
    if screens.is_empty() {
        return true;
    }
    screens.iter().any(|screen| screen.overlaps(&strip))
}

/// A rectangle in egui points, with the origin the window positions share.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    fn overlaps(&self, other: &Rect) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }
}

/// Work area of every attached monitor, in points.
///
/// The work area rather than the whole monitor: a window positioned under the
/// taskbar is as unreachable as one on a screen that has been unplugged.
#[cfg(target_os = "windows")]
fn work_areas() -> Vec<Rect> {
    use std::ffi::c_void;

    // Win32's own spelling, so the shape of each struct can be checked against
    // the documentation without translating the names first.
    #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RECT {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MONITORINFO {
        cb_size: u32,
        monitor: RECT,
        work: RECT,
        flags: u32,
    }

    // user32 is already linked by the windowing stack; naming it keeps that
    // from being an accident. The same reasoning as `input::chord_keys_down`.
    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplayMonitors(
            hdc: *mut c_void,
            clip: *const RECT,
            callback: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut RECT, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MONITORINFO) -> i32;
        fn GetDpiForSystem() -> u32;
    }

    unsafe extern "system" fn collect(
        monitor: *mut c_void,
        _hdc: *mut c_void,
        _clip: *mut RECT,
        data: isize,
    ) -> i32 {
        let out = &mut *(data as *mut Vec<RECT>);
        let mut info = MONITORINFO {
            cb_size: std::mem::size_of::<MONITORINFO>() as u32,
            monitor: RECT::default(),
            work: RECT::default(),
            flags: 0,
        };
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            out.push(info.work);
        }
        // Keep going: every monitor is wanted, not just the first.
        1
    }

    let mut found: Vec<RECT> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            collect,
            &mut found as *mut Vec<RECT> as isize,
        );
    }

    // Win32 answers in physical pixels and the settings file is in points, so
    // the areas are divided by the system scale. That is exact while every
    // monitor runs at the same scale, and approximate when they do not - which
    // is good enough for a test that only asks whether a window is on a screen
    // at all, and is why the strip it looks for is a hand's width rather than a
    // pixel.
    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let scale = if scale.is_finite() && scale > 0.1 {
        scale
    } else {
        1.0
    };
    found
        .into_iter()
        .map(|r| Rect {
            left: r.left as f32 / scale,
            top: r.top as f32 / scale,
            right: r.right as f32 / scale,
            bottom: r.bottom as f32 / scale,
        })
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn work_areas() -> Vec<Rect> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_overlap_only_where_they_really_do() {
        let screen = Rect {
            left: 0.0,
            top: 0.0,
            right: 1920.0,
            bottom: 1040.0,
        };
        let on = Rect {
            left: 1800.0,
            top: 10.0,
            right: 1920.0,
            bottom: 34.0,
        };
        let off = Rect {
            left: 1920.0,
            top: 10.0,
            right: 2040.0,
            bottom: 34.0,
        };
        assert!(screen.overlaps(&on));
        assert!(!screen.overlaps(&off), "touching edges is not overlapping");
    }

    /// A position that is not a number cannot be on any monitor, whatever the
    /// machine is running.
    #[test]
    fn a_nonsense_position_is_never_reachable() {
        assert!(!reachable([f32::NAN, 0.0], None));
        assert!(!reachable([0.0, f32::INFINITY], Some([800.0, 600.0])));
    }

    /// Whatever monitors this test is running on, the primary one has its
    /// origin at (0, 0), so a window in the top-left corner is reachable. On a
    /// platform with no implementation the same call is accepted for the other
    /// reason - there is nothing to check it against - which is the answer this
    /// asserts either way.
    #[test]
    fn the_top_left_corner_is_always_reachable() {
        assert!(reachable([20.0, 20.0], Some([800.0, 600.0])));
    }
}
