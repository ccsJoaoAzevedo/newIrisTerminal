//! The window itself: its style, its font, its size and where it sits.
//!
//! Everything here is about the frame around the terminal rather than the
//! terminal, and most of it exists because the window is drawn without the
//! system's own decorations: the geometry has to be tracked, saved and put
//! back by hand.

use super::*;

impl App {
    /// Applies theme colours *and* the style bits that are not part of
    /// `Visuals`. `set_visuals` replaces only the colours, so the scrollbar
    /// spacing has to be set separately or the switch would do nothing.
    pub(super) fn apply_style(ctx: &Context, theme: &Theme, settings: &Settings) {
        ctx.set_visuals(theme.visuals());
        ctx.style_mut(|style| {
            // Floating bars are egui's default and are the reason the
            // scrollbars read as absent: they stay a hairline until hovered.
            style.spacing.scroll.floating = !settings.show_scrollbars;
            if settings.show_scrollbars {
                style.spacing.scroll.bar_width = 10.0;
            }
        });
    }

    /// Hands the configured font to egui and records what it actually got.
    ///
    /// The setting wins over the theme's suggestion; the theme's own
    /// `font_family` is the default for anyone who has not chosen one. A name
    /// that will not load leaves the bundled monospace in place and says so,
    /// rather than being passed on to panic later.
    pub(super) fn apply_font(&mut self, ctx: &Context) {
        let wanted = if self.settings.font_family.is_empty() {
            self.theme().font_family
        } else {
            self.settings.font_family.clone()
        };

        if wanted == self.font_request {
            return;
        }
        self.font_request = wanted.clone();

        if fonts::install(ctx, &wanted) {
            self.font_family = wanted;
        } else {
            self.font_family = String::new();
            self.set_status(tr1(
                "Font {} is not installed; using the built-in monospace.",
                &format!("{wanted:?}"),
            ));
        }
    }

    /// How the terminal should be drawn, from the current settings.
    pub(super) fn render_opts(&self) -> RenderOpts {
        RenderOpts {
            font_size: self.settings.font_size,
            font_family: self.font_family.clone(),
            cursor_style: self.settings.cursor_style,
            cursor_blink: self.settings.cursor_blink,
            scrollbar: self.settings.show_scrollbars,
            syntax: self.settings.terminal_syntax_highlight,
            wrap: self.settings.wrap_lines,
            copy_on_select: self.settings.copy_on_select,
            wide_grid: true,
        }
    }

    pub fn theme(&self) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == self.settings.theme)
            .cloned()
            .unwrap_or_default()
    }

    /// Notes where the window is and how big it is, so that the values are at
    /// hand when the app exits and the window has already gone.
    ///
    /// A minimized window is skipped rather than recorded: Windows parks one
    /// off-screen at a nonsense position, and reopening there would put the app
    /// somewhere the user cannot reach it. A maximized one keeps whatever
    /// restored geometry was recorded before it was maximized, which is exactly
    /// what its own restore button would give back.
    pub(super) fn track_window_geometry(&mut self, ctx: &Context) {
        self.minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        ctx.input(|i| {
            let viewport = i.viewport();
            if viewport.minimized.unwrap_or(false) {
                return;
            }
            self.window_maximized = viewport.maximized.unwrap_or(false);
            if self.window_maximized || viewport.fullscreen.unwrap_or(false) {
                return;
            }
            if let Some(rect) = viewport.inner_rect {
                if rect.width() >= 1.0 && rect.height() >= 1.0 {
                    self.window_size = Some([rect.width(), rect.height()]);
                }
            }
            if let Some(rect) = viewport.outer_rect {
                // The off-screen parking spot again, for the platforms that do
                // not report `minimized` at all.
                if rect.min.x.is_finite() && rect.min.y.is_finite() && rect.min.x > -30_000.0 {
                    self.window_position = Some([rect.min.x, rect.min.y]);
                }
            }
        });
    }

    /// Writes the geometry back to the settings file, for whichever of the two
    /// switches is on. Called as the app exits.
    pub(super) fn persist_window_geometry(&mut self) {
        let mut changed = false;
        let settings_seen = self.settings_placement.seen;
        if self.settings.save_terminal_size {
            if self.window_size.is_some() && self.settings.window_size != self.window_size {
                self.settings.window_size = self.window_size;
                changed = true;
            }
            if self.settings.window_maximized != self.window_maximized {
                self.settings.window_maximized = self.window_maximized;
                changed = true;
            }
            // The Settings window follows the same two switches rather than
            // having a pair of its own: it is a window of the app's, and one
            // answer to "remember where my windows were" is enough.
            if settings_seen.size.is_some()
                && self.settings.settings_window_size != settings_seen.size
            {
                self.settings.settings_window_size = settings_seen.size;
                changed = true;
            }
        }
        if self.settings.save_window_position {
            if self.window_position.is_some()
                && self.settings.window_position != self.window_position
            {
                self.settings.window_position = self.window_position;
                changed = true;
            }
            if settings_seen.position.is_some()
                && self.settings.settings_window_position != settings_seen.position
            {
                self.settings.settings_window_position = settings_seen.position;
                changed = true;
            }
        }
        if changed {
            if let Err(e) = self.settings.save() {
                log::warn!("could not save the window geometry: {e}");
            }
        }
    }

    /// Nudges the window until the terminal measures exactly the geometry it
    /// was asked to open at, then stops touching it for the rest of the run.
    ///
    /// `view_cols` and `view_rows` are what the last frame actually drew, so
    /// the difference from the target converts straight into points through
    /// `cell`. The half point of slack is there because the view floors the
    /// space it is given: a window one rounding error short of the target would
    /// otherwise come out a column narrower.
    pub(super) fn fit_window(
        &mut self,
        ctx: &Context,
        view_cols: usize,
        view_rows: usize,
        cell: egui::Vec2,
    ) {
        let Some(fit) = self.fit.as_mut() else {
            return;
        };
        let target = (fit.cols as usize, fit.rows as usize);
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized || fit.attempts == 0 || (view_cols, view_rows) == target {
            let (want_cols, want_rows) = target;
            log::debug!(
                "window opened at {view_cols}x{view_rows} characters, asked for {want_cols}x{want_rows}"
            );
            self.fit = None;
            return;
        }
        let Some(inner) = ctx.input(|i| i.viewport().inner_rect) else {
            return;
        };
        if fit.recentre && fit.centre.is_none() {
            fit.centre = ctx.input(|i| i.viewport().outer_rect).map(|r| r.center());
        }
        fit.attempts -= 1;

        let size = fitted_inner_size(inner.size(), (view_cols, view_rows), target, cell);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(centre) = fit.centre {
            // The position is the frame's, the size asked for is the content's,
            // so a system title bar has to be added back before the two can be
            // put on top of each other. Zero with the app's own chrome.
            let border = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|outer| outer.size() - inner.size())
                .unwrap_or_default();
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                centre - (size + border) / 2.0,
            ));
        }
        // The measurement that says whether this worked only exists on the next
        // frame, and an idle terminal has no other reason to draw one.
        ctx.request_repaint();
    }

    /// Whether closing should stop and ask first.
    pub(super) fn should_confirm_close(&self) -> bool {
        !self.close_confirmed
            && self.settings.confirm_close_with_live_session
            && self.sessions().any(|t| t.session.is_some())
    }

    /// Asks before dropping live sessions.
    ///
    /// Honours `confirm_close_with_live_session`, which until now was a setting
    /// nothing read. It covers both routes in: the app's own close button and
    /// the window manager's, since with the system frame gone the second is
    /// often the only one left.
    pub(super) fn close_confirm_dialog(&mut self, ctx: &Context) {
        if !self.confirm_close {
            return;
        }

        let live = self.sessions().filter(|t| t.session.is_some()).count();
        let mut close_anyway = false;
        let mut cancel = false;
        let mut open = true;

        egui::Window::new(tr("Close newIrisTerminal?"))
            .id(egui::Id::new("nit-close-confirm"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(match live {
                    1 => tr("1 session is still connected.").to_string(),
                    n => tr1("{} sessions are still connected.", &n.to_string()),
                });
                ui.small(tr("Closing sends HALT to each of them."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Close anyway")).clicked() {
                        close_anyway = true;
                    }
                    if ui.button(tr("Keep working")).clicked() {
                        cancel = true;
                    }
                });
            });

        if close_anyway {
            self.confirm_close = false;
            self.close_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || !open {
            self.confirm_close = false;
        }
    }

    /// How much of the title-bar row is kept back from the tabs: the window
    /// controls and the gear, plus the strip that is always draggable.
    ///
    /// Measured rather than guessed, because a theme can hide any of the three
    /// controls and the row height decides how wide one is. The tabs are given
    /// what is left, so a long strip of them scrolls instead of running under
    /// the close button. See [`TITLE_FREE_STRIP`] for the rest of it.
    pub(super) fn title_bar_reserve(&self) -> f32 {
        // The gear is always there; the other three are the theme's to hide.
        let buttons = if self.settings.show_window_buttons {
            4
        } else {
            1
        };
        let side = TITLE_CONTROL_SIDE;
        buttons as f32 * side + TITLE_FREE_STRIP
    }

    /// The size a session for `profile` should open at.
    ///
    /// A shell gets the window's own width and IRIS the wide grid, for the
    /// reason `wide_grid` in [`terminal_view::RenderOpts`] gives. Both are
    /// only the starting point: the first frame the tab is drawn in resizes it
    /// to what the pane actually measured.
    pub(super) fn initial_size(&self, profile: &Profile) -> (u16, u16) {
        let (cols, rows) = self.terminal_size;
        if profile.is_shell() {
            let view = (self.view_size.0.max(2)).min(u16::MAX as usize) as u16;
            return (view, rows);
        }
        (cols, rows)
    }

    pub(super) fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
        self.status_at = Some(std::time::Instant::now());
    }

    /// Takes the footer message down once it has had its time, and asks for the
    /// frame that will do it.
    ///
    /// Without the repaint request the message would sit there until something
    /// else happened to draw a frame - which, at an idle prompt, is nothing at
    /// all.
    pub(super) fn expire_status(&mut self, ctx: &Context) {
        let seconds = self.settings.status_timeout_secs;
        if seconds == 0 || self.status.is_none() {
            return;
        }
        let life = std::time::Duration::from_secs(u64::from(seconds));
        let Some(at) = self.status_at else {
            return;
        };
        match life.checked_sub(at.elapsed()) {
            Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
            _ => {
                self.status = None;
                self.status_at = None;
            }
        }
    }
}
