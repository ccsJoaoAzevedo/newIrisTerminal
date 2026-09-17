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

        self.track_window_geometry(ctx);
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

        self.poll_updates();
        self.expire_status(ctx);
        self.handle_shortcuts(ctx);

        // A close asked for by the window manager - Alt+F4, or the taskbar -
        // arrives as a flag rather than an event, and has to be caught before
        // anything else gets a chance to draw over the question.
        if ctx.input(|i| i.viewport().close_requested()) && self.should_confirm_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        let mut window_action = None;
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            window_action = self.menu_bar(ui, &theme.window_buttons);
        });
        // Not when the setting has moved them into the title bar, where
        // `menu_bar` has already drawn them - that is what was putting the same
        // tabs on screen twice.
        if !self.settings.tabs_in_title_bar || self.tabs.is_empty() {
            egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_strip(ui));
        }

        if let Some(action) = window_action {
            // Close is the one action that may be refused; the rest are
            // immediate.
            if action == WindowAction::Close && self.should_confirm_close() {
                self.confirm_close = true;
            } else {
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
        if let Some(request) = panels::settings_dialog(
            ctx,
            &mut self.settings,
            &self.themes,
            &mut self.panels,
            &self.instances,
            &self.servers,
            &mut self.settings_placement,
            &theme.window_buttons,
        ) {
            requests.push(request);
        }
        // After the settings window, which is where it is opened from, and
        // before the requests are carried out, so a colour changed this frame is
        // on screen in the next one.
        let active_theme = self.settings.theme.clone();
        let theme_actions = theme_manager::theme_manager(
            ctx,
            &mut self.panels.themes,
            &mut self.themes,
            &active_theme,
            &theme.window_buttons,
        );
        // An edit to the theme in use has to show at once, which is the whole
        // point of editing it with the terminal behind the window.
        if !theme_actions.is_empty() || self.panels.themes.open {
            App::apply_style(ctx, &self.theme(), &self.settings);
        }
        for action in theme_actions {
            self.apply_theme_action(ctx, action);
        }
        // Beside the theme manager, and opened from the same section of the
        // settings window.
        let macro_actions = macro_manager::macro_manager(
            ctx,
            &mut self.panels.macros,
            &mut self.macro_groups,
            &theme.window_buttons,
        );
        for action in macro_actions {
            match action {
                macro_manager::MacroAction::Save => {
                    requests.push(UiRequest::SavePersonalMacros);
                }
                // Through the ordinary request path, so `confirm` and the
                // parameter prompt apply exactly as they do to the menu.
                macro_manager::MacroAction::Run(m) => requests.push(UiRequest::RunMacro(m)),
            }
        }

        for request in requests {
            self.handle_request(ctx, request);
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
        for tab in &mut self.tabs {
            close_down(tab);
            if let Some(split) = tab.split.as_mut() {
                close_down(&mut split.tab);
            }
        }
        self.persist_window_geometry();
    }
}
