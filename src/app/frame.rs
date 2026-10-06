//! The frame loop: what happens sixty times a second, or once, or not at all.
//!
//! `eframe` calls `update` for every frame, and this decides what the frame
//! costs and whether another one is asked for. An idle terminal must ask for
//! none: see the pacing at the end of `update`, and `pty::set_waker` for how a
//! session that has something to say wakes the loop instead of being polled.

use super::*;

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let theme = self.theme();
        let mut requests: Vec<UiRequest> = Vec::new();
        // What the terminal measured this frame, filled in once it has drawn.
        let mut fit: Option<(usize, usize, egui::Vec2)> = None;

        // egui's zoom scales every window the app opens at once, which is the
        // whole of the interface scale; the terminal undoes it in
        // `render_opts`. Only set on a change, since setting it moves the
        // window's measured size and would refit the grid every frame.
        let scale = self.ui_scale();
        if (ctx.zoom_factor() - scale).abs() > f32::EPSILON {
            ctx.set_zoom_factor(scale);
        }
        // The theme's own glass, or the app-wide value it replaced for a theme
        // written before it had one.
        let glass = theme.ui_glass.unwrap_or(self.settings.sheet_opacity);
        crate::ui::prefs::set_opacity(ctx, glass);
        self.track_window_geometry(ctx);
        crate::ui::shading::paint_backdrop(ctx);
        // Refilled as the panes draw, below, and read by the resize grips at
        // the end of the frame.
        self.pane_rects.clear();

        for tab in &mut self.tabs {
            tab.pump(&mut self.plugins);
            // A split tab holds a second session, and one that is not drained
            // would stop reading its PTY and eventually block IRIS.
            if let Some(split) = tab.split.as_mut() {
                split.tab.pump(&mut self.plugins);
            }
        }

        // Plugins may have asked for things while transforming output.
        for hook in self.plugins.take_requests() {
            match hook {
                crate::plugins::api::Hook::SendText(text) => {
                    requests.push(UiRequest::SendLines(vec![text]))
                }
                crate::plugins::api::Hook::SetStatus(text) => self.set_status(text),
                crate::plugins::api::Hook::RegisterCommand(_) => {}
            }
        }

        // Before anything reads the keyboard: the key that wakes the saver is
        // the user asking for the terminal back, not something to type into it.
        let saver_woken = self.screensaver_tick(ctx);

        self.apply_always_on_top(ctx);
        self.poll_updates();
        self.expire_status(ctx);
        self.handle_shortcuts(ctx);

        // A close asked for by the window manager - Alt+F4, or the taskbar -
        // arrives as a flag rather than an event, and has to be caught before
        // anything else gets a chance to draw over the question.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.should_close_to_tray() && self.hide_to_tray() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.should_confirm_close() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            }
        }

        let mut window_action = None;
        // Tabs in the bar run its whole height, as elementary's do: the
        // panel's own margin and the line under it would leave a band above
        // and below them that is neither tab nor terminal.
        let tabs_inline = self.settings.tabs_in_title_bar && !self.tabs.is_empty();
        let bar_frame = egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin {
            top: if tabs_inline { 0.0 } else { 2.0 },
            bottom: if tabs_inline { 0.0 } else { 2.0 },
            left: 8.0,
            right: 8.0,
        });
        egui::TopBottomPanel::top("menu")
            .frame(bar_frame)
            .show_separator_line(!tabs_inline)
            .show(ctx, |ui| {
                crate::ui::prefs::scale_style(ui, self.settings.title_bar_scale.clamp(1.0, 2.0));
                window_action = self.menu_bar(ui, &theme.window_buttons);
            });
        // Not when the setting has moved them into the title bar, where
        // `menu_bar` has already drawn them - that is what was putting the same
        // tabs on screen twice.
        if !self.settings.tabs_in_title_bar || self.tabs.is_empty() {
            egui::TopBottomPanel::top("tabs")
                .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::ZERO))
                .show_separator_line(false)
                .show(ctx, |ui| {
                    crate::ui::prefs::scale_style(
                        ui,
                        self.settings.title_bar_scale.clamp(1.0, 2.0),
                    );
                    self.tab_strip(ui)
                });
        }

        if let Some(action) = window_action {
            // Close is the one action that may be refused; the rest are
            // immediate.
            let hidden =
                action == WindowAction::Close && self.should_close_to_tray() && self.hide_to_tray();
            if hidden {
                // The sessions stay; the tray icon brings the window back.
            } else if action == WindowAction::Close && self.should_confirm_close() {
                self.confirm_close = true;
            } else if action == WindowAction::ToggleOnTop {
                self.handle_request(ctx, UiRequest::ToggleAlwaysOnTop);
            } else {
                if action == WindowAction::Minimize {
                    crate::ui::desktop::minimizing_on_purpose();
                }
                chrome::apply(ctx, action);
            }
        }

        if let Some(status) = self.status.clone() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(status);
                    if ui.small_button(tr("dismiss")).clicked() {
                        self.status = None;
                        self.status_at = None;
                    }
                });
            });
        }

        // Where the find bar floats, filled in once the central panel knows how
        // much room it has.
        let mut terminal_rect = egui::Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme.background)
                    .inner_margin(self.terminal_inset()),
            )
            .show(ctx, |ui| {
                terminal_rect = ui.max_rect();
                // A gradient goes under the margin as well as the grid. Laid
                // over the grid alone, the margin round it - the resize
                // grips' gutter, and the part of a cell the window is short
                // of - stayed the flat colour, and showed as a border.
                self.terminal_backdrop = theme.terminal_gradient().map(|gradient| {
                    let whole = terminal_rect + self.terminal_inset();
                    crate::ui::shading::ui_gradient(
                        &ui.ctx().layer_painter(ui.layer_id()),
                        whole,
                        &gradient,
                    );
                    whole
                });
                if self.tabs.is_empty() {
                    let mut open = false;
                    ui.centered_and_justified(|ui| {
                        if ui.button(tr("Open a session (Ctrl+T)")).clicked() {
                            open = true;
                        }
                    });
                    if open {
                        self.open_new_tab();
                    }
                    return;
                }

                self.active = self.active.min(self.tabs.len() - 1);
                let active = self.active;
                let focus = self.tabs[active].focus;
                let first = At {
                    tab: active,
                    pane: Pane::First,
                };
                let second = At {
                    tab: active,
                    pane: Pane::Second,
                };

                // The easter egg is not a session, and none of what follows is
                // about it: there is no character grid to fit the window to,
                // no far side to tell a width to, and nothing to drain. Its
                // board squares itself off against the pane it is given.
                if self.tabs[active].is_game() {
                    self.game_pane(ui, ctx, &theme, first);
                    return;
                }

                // `geometry` is the first pane's, which is what the window is
                // fitted from and what a new session opens at; `shown` is the
                // focused pane's, which is the size worth reporting because it
                // is the one being typed in.
                let (geometry, shown) = match self.tabs[active].split.as_ref().map(|s| s.dir) {
                    None => {
                        let measure = self.terminal_pane(ui, ctx, &theme, first, true);
                        let view = measure.view;
                        (measure, view)
                    }
                    Some(dir) => {
                        let room = ui.available_size();
                        let mut top = PaneMeasure::default();
                        let mut bottom = PaneMeasure::default();
                        let ratio = self.tabs[active].split_ratio();
                        // The panes are given exact sizes, so what the divider
                        // and the spacing around it take comes off the room
                        // first: a share that did not fit would be clipped at
                        // the window edge.
                        let mut dragged = None;
                        match dir {
                            SplitDir::Right => {
                                let gap = ui.spacing().item_spacing.x * 2.0 + SPLIT_DIVIDER;
                                let usable = room.x - gap;
                                let first_width = split_extent(usable, ratio);
                                ui.horizontal(|ui| {
                                    top = self.sized_pane(
                                        ui,
                                        ctx,
                                        &theme,
                                        first,
                                        focus == Pane::First,
                                        egui::Vec2::new(first_width, room.y),
                                    );
                                    dragged = split_divider(ui, dir, room.y, usable, ratio);
                                    bottom = self.sized_pane(
                                        ui,
                                        ctx,
                                        &theme,
                                        second,
                                        focus == Pane::Second,
                                        egui::Vec2::new(usable - first_width, room.y),
                                    );
                                });
                            }
                            SplitDir::Bottom => {
                                let gap = ui.spacing().item_spacing.y * 2.0 + SPLIT_DIVIDER;
                                let usable = room.y - gap;
                                let first_height = split_extent(usable, ratio);
                                top = self.sized_pane(
                                    ui,
                                    ctx,
                                    &theme,
                                    first,
                                    focus == Pane::First,
                                    egui::Vec2::new(room.x, first_height),
                                );
                                dragged = split_divider(ui, dir, room.x, usable, ratio);
                                bottom = self.sized_pane(
                                    ui,
                                    ctx,
                                    &theme,
                                    second,
                                    focus == Pane::Second,
                                    egui::Vec2::new(room.x, usable - first_height),
                                );
                            }
                        }
                        // Written back after both panes have been drawn: the
                        // frame the drag happened in is already laid out, and
                        // the next one opens at the new ratio.
                        if let Some(ratio) = dragged {
                            if let Some(split) = self.tabs[active].split.as_mut() {
                                split.ratio = ratio;
                            }
                        }
                        // Clicking a pane is how the keyboard is moved into it,
                        // and the strip entry says which one has it - `1:` or
                        // `2:` in front of that session name.
                        if top.clicked {
                            self.tabs[active].focus = Pane::First;
                        }
                        if bottom.clicked {
                            self.tabs[active].focus = Pane::Second;
                        }
                        let shown = if focus == Pane::Second {
                            bottom.view
                        } else {
                            top.view
                        };
                        (top, shown)
                    }
                };

                // A pane sizes its own session; every session that is not on
                // screen follows the first pane, because one left at an old
                // width would keep truncating its output at that width until it
                // was next looked at.
                //
                // Width per session rather than one for all of them: a shell
                // is told the window's width and IRIS the wide grid, and a
                // background tab has to follow its own kind or it would be
                // resized to whatever the tab on screen happens to be. See
                // `wide_grid` in [`terminal_view::RenderOpts`].
                let (_, rows) = geometry.grid;
                let view_cols = geometry.view.0;
                let minimized = self.minimized;
                let wide = terminal_view::TERMINAL_COLS.max(view_cols);
                // The wide grid rather than whatever the pane on screen was
                // told: this is what a *new* session opens at, and a shell
                // being active must not leave the next IRIS tab truncating at
                // the window width.
                if !minimized {
                    self.terminal_size =
                        (wide.min(u16::MAX as usize) as u16, geometry.grid.1 as u16);
                    self.view_size = shown;
                }
                fit = Some((geometry.view.0, geometry.view.1, geometry.cell));
                let width_for = |shell: bool| if shell { view_cols } else { wide };
                for (index, tab) in self.tabs.iter_mut().enumerate() {
                    if index == active || minimized {
                        continue;
                    }
                    tab.resize(width_for(tab.profile.is_shell()), rows);
                    if let Some(split) = tab.split.as_mut() {
                        let cols = width_for(split.tab.profile.is_shell());
                        split.tab.resize(cols, rows);
                    }
                }
            });

        self.find_bar(ctx, &theme, terminal_rect);

        // The panes have finished drawing, so the tabs can move again.
        if let Some(asked) = self.pending_layout.take() {
            match asked {
                LayoutAction::Split(index, dir) => self.split_tab(index, dir),
                LayoutAction::Unsplit(index) => self.remove_split(index),
                LayoutAction::ClosePane(at) => self.close_pane(at),
            }
        }

        self.rename_tab_dialog(ctx);
        self.close_confirm_dialog(ctx);
        if panels::update_dialog(ctx, &mut self.updates) {
            self.apply_update(ctx);
        }

        // Last, and in a foreground layer: the panels and the terminal reach
        // the window edge, and the terminal senses drags of its own.
        chrome::resize_grips(ctx, "nit-main", &self.pane_rects);

        if let Some(request) = panels::pending_macro_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::pending_native_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::usage_report_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        // The themes and the macros are edited in place, so the terminal
        // behind the window repaints in the colour being dragged; the requests
        // are carried out below, so a colour changed this frame is on screen
        // in the next one.
        let settings_was_open = self.panels.show_settings;
        requests.extend(panels::settings_dialog(
            ctx,
            &mut self.settings,
            &mut self.themes,
            &mut self.macro_groups,
            &mut self.panels,
            &self.instances,
            &self.servers,
            &mut self.settings_placement,
            &theme.window_buttons,
        ));
        // Written as the Settings window closes, not only as the app exits: an
        // app closed to the tray never exits, and the size the window was
        // left at was lost with it.
        if settings_was_open && !self.panels.show_settings {
            self.persist_window_geometry();
        }

        for request in requests {
            self.handle_request(ctx, request);
        }

        // Last of everything drawn, so it covers everything. Also on the frame
        // it was woken in, so the click that woke it lands on it and not on
        // the terminal underneath.
        if let Some(saver) = self.screensaver.as_mut() {
            let next = saver.show(ctx);
            ctx.request_repaint_after(next);
        }
        if let Some(saver) = saver_woken {
            let mut saver = saver;
            saver.show(ctx);
        } else if self.screensaver.is_none() && self.settings.screensaver.kind != Kind::None {
            // One frame at the moment it is due, which is all an idle window
            // pays for having a saver set.
            let idle = crate::ui::screensaver_view::last_activity(ctx).elapsed();
            ctx.request_repaint_after(self.settings.screensaver.wait().saturating_sub(idle));
        }

        if let Some((view_cols, view_rows, cell)) = fit.filter(|_| !self.minimized) {
            self.fit_window(ctx, view_cols, view_rows, cell);
        }

        // What is actually still moving, rather than a frame every 16 ms on the
        // chance that something is. Output wakes the loop from the reader
        // thread (`pty::set_waker`), so a session that has nothing to say costs
        // nothing to keep open.
        if self.settings.cursor_blink {
            // Only when the cursor is about to change halves. Redrawing faster
            // than that draws the same pixels.
            ctx.request_repaint_after(terminal_view::until_cursor_phase_flip(ctx));
        }
        if self.tabs.iter().any(|t| t.session.is_some()) {
            // A safety net, not the mechanism: a process can die without its
            // pipe ever closing, and `is_alive` only answers in a frame. Once a
            // second is far below anything an eye would catch and far above
            // what the old rate cost.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self.updates.working() {
            // Nothing on screen is moving, but a thread is: without a frame to
            // read it in, the check's answer and the download's progress would
            // both sit in the channel unseen. Slower than the terminal's own
            // rate, because all it has to keep up with is a number.
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Before the sessions are closed down, while every tab is still here.
        // An empty list is saved as such: closing every tab and then the
        // window is asking for nothing to come back.
        let path = config::session_path();
        if self.settings.remember_open_tabs {
            if let Err(e) = self.session_snapshot().save(&path) {
                log::warn!("could not save the open tabs: {e:#}");
            }
        } else {
            SavedSession::clear(&path);
        }
        for tab in &mut self.tabs {
            close_down(tab);
            if let Some(split) = tab.split.as_mut() {
                close_down(&mut split.tab);
            }
        }
        self.persist_window_geometry();
    }
}

use crate::features::screensaver::Kind;

impl App {
    /// Starts the screen saver once the window has been idle long enough, and
    /// stops it on the first key or movement since.
    ///
    /// Returns the saver it has just stopped, so it can be drawn one last time
    /// to catch the click that stopped it.
    fn screensaver_tick(&mut self, ctx: &Context) -> Option<crate::ui::screensaver_view::Running> {
        crate::ui::screensaver_view::note_activity(ctx);
        if let Some(saver) = &self.screensaver {
            if saver.woken(ctx) {
                // Swallowed: XP never typed the key that woke it either.
                ctx.input_mut(|i| i.events.clear());
                return self.screensaver.take();
            }
            return None;
        }
        let config = &self.settings.screensaver;
        if config.kind != Kind::None
            && !self.minimized
            && crate::ui::screensaver_view::last_activity(ctx).elapsed() >= config.wait()
        {
            self.screensaver = Some(crate::ui::screensaver_view::Running::new(config.clone()));
        }
        None
    }
}
