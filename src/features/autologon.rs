//! Unattended login.
//!
//! Driven by the *rendered screen* rather than the raw byte stream: IRIS emits
//! prompts in whatever chunks the network gives it, and matching on decoded
//! text means a prompt split across two reads is still recognised.
//!
//! Every step is bounded. A changed prompt must hand control back to the user
//! rather than leaving the tab wedged, so each state gives up after a timeout
//! and says so.

use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::term::Grid;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not configured, or already finished.
    Inactive,
    WaitUsername,
    WaitPassword,
    WaitPrompt,
    Done,
    /// Gave up; the user drives from here.
    TimedOut,
}

pub struct Autologon {
    state: State,
    username: String,
    password: Option<String>,
    post_login: Vec<String>,
    since: Instant,
    /// Grid revision at the last check, so we only re-scan on new output.
    last_revision: u64,
}

impl Autologon {
    pub fn new(profile: &Profile) -> Self {
        let active = profile.autologon_ready();
        Autologon {
            state: if active {
                State::WaitUsername
            } else {
                State::Inactive
            },
            username: profile.username.clone(),
            // Read once at construction: hitting the OS keychain on every
            // frame would be slow and, on macOS, prompt repeatedly.
            password: if active { profile.password() } else { None },
            post_login: profile.post_login.clone(),
            since: Instant::now(),
            last_revision: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.state,
            State::WaitUsername | State::WaitPassword | State::WaitPrompt
        )
    }

    /// Text for the status line, or `None` when there is nothing to say.
    pub fn status_note(&self) -> Option<String> {
        match self.state {
            State::WaitUsername => Some("Autologon: waiting for the username prompt…".into()),
            State::WaitPassword => Some("Autologon: waiting for the password prompt…".into()),
            State::WaitPrompt => Some("Autologon: waiting for the IRIS prompt…".into()),
            State::TimedOut => Some(
                "Autologon timed out — the prompt was not recognised. Continue manually.".into(),
            ),
            _ => None,
        }
    }

    /// Inspects the screen and returns whatever should be sent next.
    ///
    /// Returns `None` when there is nothing to do, which is the common case.
    pub fn observe(&mut self, grid: &Grid) -> Option<String> {
        if !self.is_running() {
            return None;
        }
        if grid.revision == self.last_revision {
            return None;
        }
        self.last_revision = grid.revision;

        if self.since.elapsed() > STEP_TIMEOUT {
            self.state = State::TimedOut;
            return None;
        }

        // Only the tail matters: an older prompt still sitting in scrollback
        // must not re-trigger a step that already ran.
        let tail = tail_text(grid, 3);

        match self.state {
            State::WaitUsername if looks_like_username_prompt(&tail) => {
                self.advance(State::WaitPassword);
                Some(format!("{}\r", self.username))
            }
            State::WaitPassword if looks_like_password_prompt(&tail) => {
                let password = self.password.take();
                match password {
                    Some(password) => {
                        self.advance(State::WaitPrompt);
                        Some(format!("{password}\r"))
                    }
                    None => {
                        // No stored secret — stop and let the user type it.
                        self.state = State::TimedOut;
                        None
                    }
                }
            }
            State::WaitPrompt if looks_like_iris_prompt(&tail) => {
                self.state = State::Done;
                if self.post_login.is_empty() {
                    None
                } else {
                    let script = self
                        .post_login
                        .iter()
                        .map(|line| format!("{line}\r"))
                        .collect::<String>();
                    Some(script)
                }
            }
            _ => None,
        }
    }

    fn advance(&mut self, next: State) {
        self.state = next;
        self.since = Instant::now();
    }
}

/// The last `n` non-empty rendered lines, lowercased for matching.
fn tail_text(grid: &Grid, n: usize) -> String {
    let mut lines: Vec<String> = grid
        .screen_text()
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(n);
    lines.drain(..start);
    lines.join("\n").to_lowercase()
}

fn looks_like_username_prompt(tail: &str) -> bool {
    ["username:", "user:", "usuário:", "usuario:"]
        .iter()
        .any(|p| tail.contains(p))
}

fn looks_like_password_prompt(tail: &str) -> bool {
    ["password:", "senha:"].iter().any(|p| tail.contains(p))
}

/// An IRIS prompt is a namespace name followed by `>`, e.g. `USER>`,
/// `%SYS>`, or `RDB1>`. Matching the last line avoids mistaking prose for it.
fn looks_like_iris_prompt(tail: &str) -> bool {
    let Some(last) = tail.lines().last() else {
        return false;
    };
    let last = last.trim_end();
    let Some(name) = last.strip_suffix('>') else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '%' | '_' | '-' | '^'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_showing(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(40, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            grid.screen[r].set_text(&line.chars().take(40).collect::<String>());
        }
        grid.touch();
        grid
    }

    fn profile() -> Profile {
        Profile {
            name: "test".into(),
            instance: "CONSISTEM".into(),
            username: "dev".into(),
            autologon: true,
            ..Profile::default()
        }
    }

    #[test]
    fn inactive_without_a_username() {
        let p = Profile {
            autologon: true,
            username: String::new(),
            ..Profile::default()
        };
        assert_eq!(Autologon::new(&p).state(), State::Inactive);
    }

    #[test]
    fn sends_the_username_at_the_username_prompt() {
        let mut auto = Autologon::new(&profile());
        let grid = grid_showing(&["Node: DEV", "Username:"]);
        assert_eq!(auto.observe(&grid).as_deref(), Some("dev\r"));
        assert_eq!(auto.state(), State::WaitPassword);
    }

    #[test]
    fn a_repeated_screen_does_not_resend() {
        let mut auto = Autologon::new(&profile());
        let grid = grid_showing(&["Username:"]);
        assert!(auto.observe(&grid).is_some());
        // Same revision — nothing new arrived.
        assert!(auto.observe(&grid).is_none());
    }

    #[test]
    fn stops_at_the_password_prompt_when_no_secret_is_stored() {
        let mut auto = Autologon::new(&profile());
        auto.state = State::WaitPassword;
        auto.password = None;
        let grid = grid_showing(&["Password:"]);
        assert!(auto.observe(&grid).is_none());
        assert_eq!(auto.state(), State::TimedOut);
    }

    #[test]
    fn post_login_commands_run_once_the_prompt_appears() {
        let mut auto = Autologon::new(&Profile {
            post_login: vec!["ZN \"%SYS\"".into()],
            ..profile()
        });
        auto.state = State::WaitPrompt;
        let grid = grid_showing(&["USER>"]);
        assert_eq!(auto.observe(&grid).as_deref(), Some("ZN \"%SYS\"\r"));
        assert_eq!(auto.state(), State::Done);
    }

    #[test]
    fn prompt_detection_accepts_real_iris_prompts() {
        assert!(looks_like_iris_prompt("user>"));
        assert!(looks_like_iris_prompt("%sys>"));
        assert!(looks_like_iris_prompt("rdb1>"));
    }

    #[test]
    fn prompt_detection_rejects_prose_ending_in_a_bracket() {
        assert!(!looks_like_iris_prompt("press enter to continue >"));
        assert!(!looks_like_iris_prompt("nothing here"));
    }

    #[test]
    fn portuguese_prompts_are_recognised() {
        assert!(looks_like_username_prompt("usuário:"));
        assert!(looks_like_password_prompt("senha:"));
    }
}
