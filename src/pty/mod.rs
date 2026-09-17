//! Sessions: the three ways this terminal reaches something to talk to.
//!
//! A [`Session`] is a local IRIS process, a local shell, or a Telnet login to a
//! remote server. Which one a tab gets is decided by its `Profile`; above this
//! point they behave identically.
//!
//! - [`launcher`] finds the IRIS instances installed on this machine and how to
//!   start a terminal into one.
//! - [`session`] owns the child process and its pseudo-terminal.
//! - [`telnet`] speaks enough of the protocol to log in and carry a session.
//!
//! Every session reads on a thread of its own and pushes what it reads down a
//! channel. The UI never blocks on that: it is a repaint loop, so the reader
//! wakes it instead - see [`set_waker`]. Getting this wrong costs a core, since
//! the only alternative is to redraw continuously and poll.

pub mod launcher;
pub mod session;
pub mod telnet;

pub use session::{PtySession, Session};

use std::sync::{Mutex, OnceLock};

type Waker = Box<dyn Fn() + Send + Sync>;

/// Whatever wants to be told that a session produced output.
///
/// The reader threads have no way back to the UI, and the UI has no way to
/// block on them - it is a repaint loop. Without this the only way for output
/// to ever reach the screen is to redraw continuously and poll the channel,
/// which is what an idle terminal used to spend a core's worth of time on.
/// One waker for the whole process, set once at startup, because every session
/// wakes the same event loop.
fn waker() -> &'static Mutex<Option<Waker>> {
    static WAKER: OnceLock<Mutex<Option<Waker>>> = OnceLock::new();
    WAKER.get_or_init(|| Mutex::new(None))
}

/// Registers the callback that [`wake`] calls. Replaces any previous one.
pub fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut slot) = waker().lock() {
        *slot = Some(Box::new(wake));
    }
}

/// Asks for a frame, from whichever reader thread has just pushed bytes.
///
/// Silent when nothing is registered - the terminal core is used headless by
/// the tests, and there is no UI there to wake.
pub fn wake() {
    if let Ok(slot) = waker().lock() {
        if let Some(wake) = slot.as_ref() {
            wake();
        }
    }
}
