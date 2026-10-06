//! Checking for a newer version, and installing one.
//!
//! The check and the download both run on threads; see [`UpdateState`] for the
//! handover. Nothing here may block a frame.
//!
//! [`UpdateState`]: super::UpdateState

use super::*;

/// What the app knows about a newer version.
///
/// The check runs on a thread, so this is the state machine between "asked" and
/// "the user has decided": nothing here blocks a frame.
#[derive(Default)]
pub struct UpdateState {
    pub(super) events: Option<crossbeam_channel::Receiver<update::Event>>,
    /// A release newer than this build, once one has been found.
    pub available: Option<update::Release>,
    /// The downloaded executable, waiting to be put in place.
    pub staged: Option<std::path::PathBuf>,
    pub downloading: bool,
    /// Bytes of the download written so far, shared with the thread doing the
    /// writing. Read every frame while `downloading`, which is what lets the
    /// dialog say how far it has got instead of only that it is trying.
    pub(super) progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub error: Option<String>,
    /// The dialog has been shown for this release and dismissed. Kept so a
    /// "later" is not undone by the next frame.
    pub asked: bool,
    /// The user asked for this check, so its answer is worth a status line even
    /// when the answer is "nothing new".
    pub(super) announce: bool,
}

impl UpdateState {
    /// Starts a check on a thread of its own.
    pub(super) fn start_check(&mut self) {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.error = None;
        update::check_in_background(tx);
    }

    /// Starts a check the user asked for, whose answer is always reported.
    pub fn check_now(&mut self) {
        self.announce = true;
        self.start_check();
    }

    /// Starts downloading the release that was found.
    pub fn start_download(&mut self) {
        let Some(release) = self.available.clone() else {
            return;
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.downloading = true;
        self.error = None;
        self.progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        update::download_in_background(release, self.progress.clone(), tx);
    }

    /// Bytes written so far, and the total if the release said how big it is.
    pub fn downloaded(&self) -> (u64, u64) {
        (
            self.progress.load(std::sync::atomic::Ordering::Relaxed),
            self.available.as_ref().map(|r| r.size).unwrap_or(0),
        )
    }

    /// Whether a thread is still working, so the frame loop knows to keep
    /// drawing.
    ///
    /// An idle terminal gives egui no reason to draw another frame, and the
    /// events from the check and the download are only picked up while frames
    /// are being drawn - so without this the answer could sit in the channel
    /// unread and the spinner never move.
    pub(super) fn working(&self) -> bool {
        self.events.is_some()
    }

    pub(super) fn drain(&mut self) -> Vec<update::Event> {
        let Some(rx) = self.events.as_ref() else {
            return Vec::new();
        };
        rx.try_iter().collect()
    }
}

impl App {
    /// Takes whatever the update thread has said since the last frame.
    pub(super) fn poll_updates(&mut self) {
        for event in self.updates.drain() {
            match event {
                update::Event::Available(release) => {
                    self.updates.available = Some(release);
                    self.updates.asked = true;
                }
                update::Event::UpToDate => {
                    // Only worth saying when the user asked the question.
                    if self.updates.announce {
                        self.set_status(tr1("This is the newest version ({}).", update::CURRENT));
                    }
                }
                update::Event::Downloaded(path) => {
                    self.updates.downloading = false;
                    self.updates.staged = Some(path);
                }
                update::Event::Failed(why) => {
                    let downloading = self.updates.downloading;
                    self.updates.downloading = false;
                    if downloading {
                        // The failure this used to swallow. A download that
                        // stopped left the dialog showing its Download button
                        // again with nothing said, which reads exactly like a
                        // button that does nothing - and was how a timeout on
                        // a slow connection looked. It is the user's problem
                        // now: they pressed the button.
                        self.updates.error = Some(why.clone());
                        self.set_status(tr1("Could not download the update: {}", &why));
                    } else if self.updates.announce {
                        // A failed check at startup is not the user's problem:
                        // it goes to the log. One they asked for is answered.
                        self.set_status(tr1("Could not check for updates: {}", &why));
                    } else {
                        log::warn!("update check failed: {why}");
                    }
                }
            }
            self.updates.announce = false;
            // The exchange is over, so nothing is left to poll for: this is
            // what stops `working` from keeping the frame loop awake for the
            // rest of the session.
            self.updates.events = None;
        }
    }

    /// Installs and restarts outright, without the live-session question a
    /// plain close asks.
    ///
    /// "Restart and update" is already an answer to that question: it names
    /// the close it is asking for. Putting the confirmation in front of it
    /// meant the one button whose whole point is "close and come back" stopped
    /// to ask whether to close, which read as the update having failed.
    /// `confirm_close_with_live_session` guards the closes the user did not
    /// spell out - the window's X, Alt+F4 - and this is not one of them.
    pub(super) fn apply_update(&mut self, ctx: &Context) {
        if self.updates.staged.is_none() {
            return;
        }
        self.install_and_restart(ctx);
    }

    /// Puts the downloaded build in place and closes, so the copy that is
    /// starting takes over.
    pub(super) fn install_and_restart(&mut self, ctx: &Context) {
        let Some(staged) = self.updates.staged.clone() else {
            return;
        };
        match update::install(&staged) {
            Ok(()) => {
                self.updates.available = None;
                self.updates.staged = None;
                // The new build is on disk and this process is the old one:
                // the close that follows is the update, not a close to be
                // reconsidered, so it must not stop to ask.
                self.close_confirmed = true;
                // And a confirmation already on screen - raised by an X the
                // user clicked and then thought better of - is not left
                // hanging over a window that is going away.
                self.confirm_close = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.updates.error = Some(format!("{e:#}"));
                self.set_status(tr1("Could not apply the update: {}", &format!("{e:#}")));
            }
        }
    }
}

impl App {
    /// Records the version now running, and offers the usage report when it
    /// is newer than the one that ran last - the first start after an update.
    ///
    /// Not on a first install: there is nothing yet for a report to say.
    pub(super) fn note_version_started(&mut self) {
        let last = std::mem::replace(&mut self.settings.last_version, update::CURRENT.to_string());
        if last == update::CURRENT {
            return;
        }
        let updated = !last.is_empty() && update::is_newer(update::CURRENT, &last);
        if let Err(e) = self.settings.save() {
            log::warn!("could not record the version that started: {e:#}");
        }
        if updated && self.settings.ask_usage_report {
            self.panels.usage = Some(crate::ui::panels::UsageOffer {
                report: self.usage_report(),
                email: self.settings.usage_report_email.clone(),
            });
        }
    }

    fn usage_report(&self) -> String {
        use crate::features::macros::Origin;
        let count = |origin: Origin| {
            self.macro_groups
                .iter()
                .filter(|g| g.origin == origin)
                .map(|g| g.macros.len())
                .sum::<usize>()
        };
        let inventory = crate::features::usage::Inventory {
            personal_macros: count(Origin::Personal),
            organisation_macros: self
                .settings
                .org_macros()
                .map(|_| count(Origin::Organization)),
            own_themes: self.themes.iter().filter(|t| !t.builtin).count(),
        };
        crate::features::usage::report(&self.settings, &inventory)
    }

    /// Puts the usage report into an e-mail to `to`, in the user's own mail
    /// program, for them to send.
    ///
    /// The report goes on the clipboard as well, whatever else happens: a mail
    /// program cuts a long `mailto:` link short, and some machines have no
    /// program set to open one at all - and either way, pasting it is a
    /// gesture everyone already knows.
    pub(super) fn send_usage_report(&mut self, ctx: &Context, to: String) {
        use crate::features::usage;
        if self.settings.usage_report_email != to {
            self.settings.usage_report_email = to.clone();
            if let Err(e) = self.settings.save() {
                self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
            }
        }
        let report = self.usage_report();
        ctx.copy_text(report.clone());
        let subject = format!("newIrisTerminal {} - usage report", update::CURRENT);
        let mut link = usage::mailto(&to, &subject, &report);
        if link.len() > usage::MAILTO_LIMIT {
            link = usage::mailto(
                &to,
                &subject,
                tr("The report is on the clipboard: paste it here (Ctrl+V)."),
            );
        }
        match usage::open(&link) {
            Ok(()) => self.set_status(tr(
                "The report is ready to send in your mail program. It is on the clipboard too.",
            )),
            Err(e) => self.set_status(tr1(
                "No mail program could be opened ({}). The report is on the clipboard: paste it into an e-mail.",
                &format!("{e:#}"),
            )),
        }
    }
}
