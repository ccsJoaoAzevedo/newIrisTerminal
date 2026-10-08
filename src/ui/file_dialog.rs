//! The system's Open dialog, for the Settings rows that take a file.
//!
//! It runs on a thread of its own: modal on the UI thread, it would stop every
//! session's output being drained for as long as it stayed up.

use std::sync::mpsc::Receiver;

use egui::Context;

/// What kind of file a dialog is for: the name its filter is shown under, and
/// the patterns it lets through, separated by `;` (`*.png;*.gif`).
#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub name: &'static str,
    pub patterns: &'static str,
}

/// Opens the dialog for a file of `kind`, starting at `current`; the answer
/// arrives on the receiver, `None` if it was cancelled. The window is woken
/// when it does, so whoever holds the receiver only has to look at it on the
/// frames it is drawn anyway.
pub fn pick(
    ctx: &Context,
    title: &'static str,
    kind: Kind,
    current: &str,
) -> Receiver<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let current = current.trim().to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(open(title, kind, &current));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    });
    rx
}

/// Whether there is a dialog to offer at all. Elsewhere the path is typed.
pub const AVAILABLE: bool = cfg!(windows);

#[cfg(windows)]
fn open(title: &str, kind: Kind, current: &str) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // Pairs of name and pattern, each ended by a NUL, and the list by one more.
    let shown = kind.patterns.replace(';', ", ");
    let filter: Vec<u16> = format!("{} ({shown})\0{}\0\0", kind.name, kind.patterns)
        .encode_utf16()
        .collect();
    let title = wide(title);
    let mut file = vec![0u16; 4096];
    // Starting from the file already chosen opens the dialog in its folder.
    for (slot, unit) in file.iter_mut().zip(current.encode_utf16().take(4095)) {
        *slot = unit;
    }
    // SAFETY: OPENFILENAMEW is plain data - integers, pointers and an
    // optional callback - for which all zeroes is the documented "unset".
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    // NOCHANGEDIR: left to itself the dialog moves the whole process's
    // working directory to wherever the file was.
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: every buffer the struct points at outlives the call.
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(String::from_utf16_lossy(&file[..end]))
}

#[cfg(not(windows))]
fn open(_title: &str, _kind: Kind, _current: &str) -> Option<String> {
    None
}

/// Takes the answer off `picking` if it has come, leaving it `None` once it
/// has - or once the dialog's thread has gone without one.
pub fn collect(ctx: &Context, picking: &mut Option<Receiver<Option<String>>>) -> Option<String> {
    let rx = picking.as_ref()?;
    match rx.try_recv() {
        Ok(picked) => {
            *picking = None;
            picked
        }
        // Often enough to see its answer, and no more: the thread wakes the
        // window itself when it has one.
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after_for(
                std::time::Duration::from_millis(250),
                egui::ViewportId::ROOT,
            );
            None
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            *picking = None;
            None
        }
    }
}
