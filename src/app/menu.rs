//! The menu bar, the keyboard shortcuts, and the requests they raise.
//!
//! Menu items and shortcuts do not act directly: both raise a [`UiRequest`],
//! which `handle_request` carries out. One path means a gesture behaves the
//! same however it was reached, and that a confirmation or a parameter prompt
//! only has to be written once.
//!
//! [`UiRequest`]: crate::ui::panels::UiRequest

use super::*;

impl App {
    /// Draws the find bar over the terminal, when there is one to draw.
    ///
    /// Floated over the output rather than given a strip of the window: a strip
    /// would take rows away from the grid, and changing the grid's height
    /// resizes the pseudoconsole and makes IRIS repaint - so opening the find
    /// bar would disturb the very screen it was opened to read.
    pub(super) fn find_bar(&mut self, ctx: &Context, theme: &Theme, over: egui::Rect) {
        let active = self.active;
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let focus = tab.focus;
        let Some(pane) = tab.pane_mut(focus) else {
            return;
        };
        if !pane.view.search.open {
            return;
        }

        // Against this frame's grid, so the hits the bar counts are the hits
        // the terminal has just drawn. Skipped when nothing has changed - see
        // `Search::refresh`.
        let search = &mut pane.view.search;
        search.refresh(&pane.grid);

        let mut action = None;
        egui::Area::new(egui::Id::new(("nit-find", active)))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                (over.right() - FIND_BAR_WIDTH).max(over.left()),
                over.top() + 6.0,
            ))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(theme.background)
                    .show(ui, |ui| {
                        action = crate::ui::search::bar(ui, search, theme);
                    });
            });

        match action {
            Some(crate::ui::search::Action::Close) => pane.view.search.close(),
            Some(crate::ui::search::Action::Reveal) => {
                if let Some(hit) = pane.view.search.current_match() {
                    pane.view.reveal = Some((hit.line, hit.from));
                }
            }
            None => {}
        }
        // The bar is drawn after the grid it highlights, so a query typed this
        // frame is only painted over the output on the next one. An idle
        // terminal draws no frame of its own, so one has to be asked for.
        ctx.request_repaint();
    }

    /// Whether the active terminal currently owns the keyboard.
    ///
    /// Macro shortcuts are gated on this. `Ctrl+Shift+G` is a shortcut when
    /// the terminal has focus and an ordinary editing gesture when the cursor
    /// is in a text field, and only the widget with focus can say which.
    pub(super) fn terminal_has_focus(&self, ctx: &Context) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let id = egui::Id::new(("nit-terminal", tab.uid));
        ctx.memory(|m| m.focused() == Some(id))
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &Context) {
        // The macro editor is listening for a chord: Ctrl+T there means "bind
        // this macro to Ctrl+T", and opening a tab instead would make the app's
        // own shortcuts the only ones that could never be recorded.
        if self.panels.macros.capture_shortcut || self.panels.capture_manager_shortcut {
            return;
        }
        // Nor while the window is not the active one: a macro shortcut sends
        // its lines to a live session, so it answers to the same rule the
        // keyboard does - see [`App::terminal_pane`].
        if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let new_tab = consume_exact(ctx, cmd, Key::T);
        let close_tab = consume_exact(ctx, cmd, Key::W);
        let next_tab = consume_exact(ctx, cmd, Key::Tab);
        let zoom_in = consume_exact(ctx, cmd, Key::Plus) | consume_exact(ctx, cmd, Key::Equals);
        let zoom_out = consume_exact(ctx, cmd, Key::Minus);

        let mut jump = None;
        for (n, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ]
        .iter()
        .enumerate()
        {
            if consume_exact(ctx, cmd, *key) {
                jump = Some(n);
            }
        }

        if new_tab {
            self.open_new_tab();
        }
        if close_tab && !self.tabs.is_empty() {
            self.close_tab(self.active);
        }
        if next_tab && !self.tabs.is_empty() {
            self.activate_tab((self.active + 1) % self.tabs.len());
        }
        if let Some(n) = jump {
            self.activate_tab(n);
        }
        // Font size drives cell size, which drives grid dimensions, so zooming
        // reflows and resizes the PTY through the ordinary resize path.
        if zoom_in || zoom_out {
            let delta = if zoom_in { 1.0 } else { -1.0 };
            self.settings.font_size = (self.settings.font_size + delta).clamp(8.0, 28.0);
            let _ = self.settings.save();
        }

        // Ctrl+Delete: reset the terminal and drop the history, the one
        // gesture that is meant to destroy the transcript. Gated on the
        // terminal having focus so it stays "delete word" in a text field.
        let terminal_focus = self.terminal_has_focus(ctx);

        // Ctrl+F searches this tab's transcript. Taken while the terminal has
        // focus and also while the find bar already has it, because Ctrl+F in
        // an editor starts a fresh search rather than doing nothing the second
        // time - and no text field the app has uses the chord for anything.
        let find_open = self.focused_view().is_some_and(|view| view.search.open);
        if (terminal_focus || find_open) && consume_exact(ctx, cmd, Key::F) {
            if let Some(view) = self.focused_view_mut() {
                view.search.open();
            }
        }
        // F3 walks the hits without going back to the bar first, which is the
        // other half of how every editor does this. Only once a search has been
        // opened: F3 on its own is a key IRIS is entitled to.
        if find_open {
            let forward = consume_exact(ctx, Modifiers::NONE, Key::F3);
            let back = consume_exact(ctx, Modifiers::SHIFT, Key::F3);
            if forward || back {
                if let Some(view) = self.focused_view_mut() {
                    view.search.step(forward);
                    if let Some(hit) = view.search.current_match() {
                        view.reveal = Some((hit.line, hit.from));
                    }
                }
            }
        }
        if terminal_focus && consume_exact(ctx, Modifiers::CTRL, Key::Delete) {
            self.clear_active_terminal();
        }

        // The macro manager's own chord, if the user has set one. Before the
        // macros themselves so that a chord bound to both opens the manager -
        // the one of the two that cannot send anything to a live session.
        if terminal_focus {
            if let Some((modifiers, key)) = self
                .settings
                .macro_manager_shortcut
                .as_deref()
                .and_then(shortcut::parse)
            {
                if consume_exact(ctx, modifiers, key) {
                    self.panels.macros.open = true;
                }
            }
        }

        // Macro shortcuts come after the app's own, which is what the editor's
        // "the app already uses this" warning promises. A macro whose `key` is
        // missing or unparseable simply never fires; the text is shared and
        // hand-edited, so it cannot be trusted to mean anything.
        if terminal_focus {
            let mut fire = None;
            for group in &self.macro_groups {
                for m in &group.macros {
                    let Some((modifiers, key)) = m.key.as_deref().and_then(shortcut::parse) else {
                        continue;
                    };
                    if consume_exact(ctx, modifiers, key) {
                        fire = Some(m.clone());
                    }
                }
            }
            // Routed through the ordinary request path, so `confirm` and
            // parameter prompting apply exactly as they do to a click.
            if let Some(m) = fire {
                self.handle_request(ctx, UiRequest::RunMacro(m));
            }
        }
    }

    /// The top row: app controls on the left, and - when the app is drawing its
    /// own frame - the window buttons and the draggable area on the right.
    pub(super) fn menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        buttons: &crate::config::theme::WindowButtons,
    ) -> Option<WindowAction> {
        let mut action = None;
        // Opening a tab has to happen after the closure: it borrows `self`
        // mutably, and the bar is already holding it.
        let mut open_default = false;
        let mut pick: Option<Profile> = None;
        // The system bar brings its own controls, and the setting can turn the
        // app's off entirely.
        let own_buttons = self.settings.show_window_buttons;
        // Whether the tabs share this row. Read before the closure: it decides
        // both what goes in the middle of the bar and whether the session's own
        // line is drawn at all.
        let inline_tabs = self.settings.tabs_in_title_bar && !self.tabs.is_empty();
        // Taken out of `self` for the length of the row, because the gear is
        // handed to `chrome` as a `&mut bool` while the closure still holds
        // `self` for the tab strip.
        let mut show_settings = self.panels.show_settings;
        // Which end the gear goes on: beside minimize, which means the leading
        // group when that group is the one being drawn. Worked out once,
        // because getting it from each end separately is how the gear ended up
        // on neither - a theme with left-hand buttons and the buttons switched
        // off drew no leading group for it to join and skipped the trailing
        // one because the theme said left. Settings was then unreachable.
        let leading_buttons = own_buttons && buttons.left;
        ui.horizontal(|ui| {
            // Drawn before anything else when the theme puts them on the left,
            // which is where Aqua has them.
            if leading_buttons {
                if let Some(asked) =
                    chrome::leading_window_buttons(ui, buttons, Some(&mut show_settings))
                {
                    action = Some(asked);
                }
                ui.add_space(6.0);
            }
            let endpoint = self.new_tab_profile.endpoint();
            let new_tab = icon_button(ui, icons::Glyph::Plus, buttons.new_tab).on_hover_text(tr1(
                "New session on {} (Ctrl+T).\nRight-click to connect somewhere else.",
                &endpoint,
            ));
            let new_tab_right = new_tab.rect.right();
            if new_tab.clicked() {
                open_default = true;
            }
            // The escape hatch that replaces the dialog: everything it used to
            // offer - the profiles and the instances found on this machine -
            // one click away instead of in front of every new session.
            new_tab.context_menu(|ui| {
                // The launcher's servers first: they are the whole reason this
                // menu is worth opening, and the preferred one is already what
                // the button does.
                if !self.servers.is_empty() {
                    ui.weak(tr("IRIS servers"));
                    let preferred = self.new_tab_profile.name.clone();
                    for server in self.servers.others(Some(&preferred)) {
                        let mut button = ui.button(server.menu_label());
                        // What the entry actually does, since a local server
                        // opens an instance and a remote one asks for a login.
                        let hint = match server.target(&self.instances) {
                            crate::config::servers::Target::Local { instance } => {
                                tr1("Local session on instance {}", &instance)
                            }
                            crate::config::servers::Target::Telnet { address, port } => {
                                tr1("Telnet login to {}", &format!("{address}:{port}"))
                            }
                        };
                        let hint = if server.comment.trim().is_empty() {
                            hint
                        } else {
                            format!("{hint}\n{}", server.comment.trim())
                        };
                        button = button.on_hover_text(hint);
                        if button.clicked() {
                            pick = Some(Profile::for_server(
                                server,
                                &self.instances,
                                &self.new_tab_profile,
                            ));
                            ui.close_menu();
                        }
                    }
                    let loose_instances = self
                        .instances
                        .iter()
                        .any(|name| !self.servers.covers_instance(&self.instances, name));
                    if !self.settings.profiles.is_empty() || loose_instances {
                        ui.separator();
                    }
                }
                for profile in &self.settings.profiles {
                    let label = if profile.instance.is_empty() {
                        profile.name.clone()
                    } else {
                        format!("{}  ({})", profile.name, profile.instance)
                    };
                    if ui.button(label).clicked() {
                        pick = Some(profile.clone());
                        ui.close_menu();
                    }
                }
                if !self.settings.profiles.is_empty() && !self.instances.is_empty() {
                    ui.separator();
                }
                for name in &self.instances {
                    // Skipped when a server entry already opens it: the same
                    // session under two names is not a choice.
                    if self.servers.covers_instance(&self.instances, name) {
                        continue;
                    }
                    if ui.button(name).clicked() {
                        // Built through the same path as a server, which is what
                        // guarantees a local instance opens locally. Inheriting
                        // the current profile wholesale used to carry its
                        // `remote` across, so picking the instance `CONSISTEM`
                        // while a Telnet tab was current opened Telnet again.
                        pick = Some(Profile::for_server(
                            &crate::config::servers::Server::for_instance(name),
                            &self.instances,
                            &self.new_tab_profile,
                        ));
                        ui.close_menu();
                    }
                }
                if self.settings.profiles.is_empty()
                    && self.instances.is_empty()
                    && self.servers.is_empty()
                {
                    ui.weak(tr("No servers, profiles or instances found."));
                }

                // The shells this machine has, under the IRIS entries rather
                // than among them: they are a different kind of session, and
                // nothing about a profile or a namespace applies to one. See
                // [`crate::plugins::shells`].
                let shells = crate::plugins::shells::available();
                if !shells.is_empty() {
                    ui.separator();
                    ui.weak(tr("Shells"));
                    for shell in &shells {
                        if ui
                            .button(&shell.name)
                            .on_hover_text(shell.command_line())
                            .clicked()
                        {
                            pick = Some(Profile::for_shell(shell));
                            ui.close_menu();
                        }
                    }
                }
            });
            // One rule after the new-session button, and none at all when the
            // tabs are up here: the first tab's own edge is the divider, and a
            // rule in front of it - there used to be two, with a gap between
            // them - reads as a slot with something missing out of it.
            //
            // Macros, Export and the IRIS utilities all used to sit between
            // those two rules. Every one of them was about the output rather
            // than about the app, and all three are now on the terminal's own
            // right-click menu, beside the session they act on; writing macros
            // is in Settings, with the themes.
            if !inline_tabs {
                ui.separator();
            }

            // The tabs, when the setting has moved them up here. Drawn in the
            // row rather than into a rectangle handed to `chrome`, which is
            // what lets the row's own cursor measure them: everything after
            // them is then the space they left, and that space is what the
            // window is dragged by.
            if inline_tabs {
                // Bounded, or a strip of tabs long enough would run under the
                // window buttons: a scroll area takes the width it is offered,
                // and what is offered here has the buttons at the end of it.
                // The reserve also leaves a strip in front of them that is
                // always free, so a row filled with tabs still has somewhere
                // to take hold of the window.
                let tabs_from = ui.cursor().min.x;
                let room = ui.available_width() - self.title_bar_reserve();
                self.tab_strip_bounded(ui, room.max(60.0));
                // The sliver between the button and the first tab. Nothing is
                // allocated for it - it is the row's own spacing - it simply
                // drags the window now instead of doing nothing.
                if let Some(asked) = chrome::drag_span(
                    ui,
                    new_tab_right..tabs_from,
                    true,
                    "nit-main",
                    "before-tabs",
                ) {
                    action = Some(asked);
                }
            }

            // The session's own line - instance, PID, geometry - unless the
            // tabs have moved up here, in which case the row is theirs: the
            // two cannot both have the middle of the bar, and the tabs say
            // which session it is anyway.
            if !inline_tabs {
                if let Some(tab) = self.active_tab() {
                    let (cols, rows) = self.view_size;
                    // The instance, then what identifies this session of it,
                    // then how big the window is - in that order because that
                    // is how specific each one is.
                    let pid = match tab.pid().filter(|_| self.settings.show_pid) {
                        Some(pid) => format!("  PID {pid}"),
                        None => String::new(),
                    };
                    let info = format!("{}{pid}  {cols}x{rows}", tab.profile.endpoint());
                    // Reading matter, and nothing else: the window is dragged
                    // by it like any other empty stretch of the bar.
                    if let Some(asked) = chrome::drag_text(
                        ui,
                        egui::RichText::new(info).weak(),
                        true,
                        "nit-main",
                        "session",
                    ) {
                        action = Some(asked);
                    }
                }
            }

            // Last, so the space it claims for dragging is whatever the items
            // above did not take. Claimed even when the window controls are
            // hidden or already drawn on the left: without it there is nothing
            // to drag the window by.
            let trailing = own_buttons && !buttons.left;
            let gear = (!leading_buttons).then_some(&mut show_settings);
            if let Some(asked) = chrome::title_bar_controls(ui, buttons, trailing, "nit-main", gear)
            {
                action = Some(asked);
            }
        });
        self.panels.show_settings = show_settings;

        // A pick opens that session and leaves the default alone. Making the
        // choice stick was worse than it sounds: after one Telnet server, the
        // plain "+" kept reconnecting to it, and there was no longer any way to
        // get back to the preferred server except through the menu.
        if let Some(profile) = pick {
            self.open_tab(profile);
        } else if open_default {
            self.open_new_tab();
        }
        action
    }

    /// Carries out whatever a panel asked for.
    pub(super) fn handle_request(&mut self, ctx: &Context, request: UiRequest) {
        match request {
            UiRequest::RunMacro(m) => {
                // Parameters or a confirmation flag both mean "ask first".
                if m.needs_input() || m.confirm {
                    self.panels.pending = Some(PendingMacro::new(m));
                } else {
                    let lines = m.expand(&[]);
                    self.send_lines_to_active(&lines);
                }
            }
            UiRequest::RunNative(native, values) => {
                let invocation = native.build(&values);
                self.send_lines_to_active(&invocation.lines);
                self.set_status(invocation.summary);
            }
            UiRequest::SendLines(lines) => self.send_lines_to_active(&lines),

            UiRequest::ExportText(range) => self.export(range, export::Format::Text),
            UiRequest::ExportHtml(range) => self.export(range, export::Format::Html),
            UiRequest::CopyRange(range) => {
                if let Some(tab) = self.active_tab() {
                    let text = export::to_text(&tab.grid, range);
                    ctx.copy_text(text);
                    self.set_status(tr("Copied to the clipboard."));
                }
            }

            UiRequest::SettingsChanged => {
                // The style is reapplied either way: a failed write still has
                // to be reflected on screen, or the UI would disagree with the
                // settings the user just changed.
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
                self.history.set_persist(
                    &config::command_history_path(),
                    self.settings.save_command_history,
                );
                // The next check has to authenticate as whoever the field now
                // names, not as whoever it named when the app started.
                update::configure_proxy_user(&self.settings.proxy_user);
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
            }
            UiRequest::OpenFolder(path) => {
                if let Err(e) = config::open_in_file_manager(&path) {
                    self.set_status(tr2(
                        "Could not open {}: {}",
                        &path.display().to_string(),
                        &format!("{e:#}"),
                    ));
                }
            }
            UiRequest::CheckForUpdates => self.updates.check_now(),
            UiRequest::SetProxyPassword(password) => match update::set_proxy_password(&password) {
                Ok(()) if password.is_empty() => self.set_status(tr("Proxy password forgotten.")),
                Ok(()) => self.set_status(tr("Proxy password saved.")),
                Err(e) => self.set_status(tr1(
                    "Could not save the proxy password: {}",
                    &format!("{e:#}"),
                )),
            },
            UiRequest::SavePersonalMacros => {
                let path = config::personal_macros_path();
                let xml = macros::to_xml(&self.macro_groups);
                match std::fs::write(&path, xml) {
                    Ok(()) => self.set_status(tr1(
                        "Saved personal macros to {}",
                        &path.display().to_string(),
                    )),
                    Err(e) => self.set_status(tr1("Could not save macros: {}", &format!("{e:#}"))),
                }
            }
        }
    }

    /// Carries out what the theme manager asked for.
    ///
    /// The manager edits the themes in place so the window behind it repaints
    /// as a colour is dragged; this is the half that reaches the disk, which a
    /// paint pass has no business doing.
    pub(super) fn apply_theme_action(&mut self, ctx: &Context, action: ThemeAction) {
        match action {
            ThemeAction::Activate(name) => {
                self.settings.theme = name;
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
            }
            ThemeAction::Save(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                // A built-in lives in the binary. Nothing here can edit one, so
                // nothing here writes one out either.
                if self.themes[index].builtin {
                    return;
                }
                let path = match self.themes[index].path.clone() {
                    Some(path) => path,
                    None => {
                        let path = config::theme_path_for(&name);
                        self.themes[index].path = Some(path.clone());
                        path
                    }
                };
                if let Err(e) = config::save_theme(&self.themes[index], &path) {
                    self.set_status(tr1("Could not save theme: {}", &format!("{e:#}")));
                }
            }
            ThemeAction::Delete(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                if self.themes[index].builtin {
                    return;
                }
                let removed = self.themes.remove(index);
                if let Some(path) = removed.path.as_ref() {
                    if let Err(e) = std::fs::remove_file(path) {
                        self.set_status(tr2(
                            "Could not delete {}: {}",
                            &path.display().to_string(),
                            &e.to_string(),
                        ));
                    }
                }
                // Deleting the theme in use has to leave something on screen.
                if self.settings.theme == name {
                    let fallback = self
                        .themes
                        .first()
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    self.apply_theme_action(ctx, ThemeAction::Activate(fallback));
                } else {
                    self.set_status(tr1("Deleted theme {}.", &name));
                }
            }
        }
    }
}
