//! Tabs and the panes inside them: opening, closing, splitting, choosing.
//!
//! A tab holds one session, or two when it is split. [`At`] is how the rest of
//! the app names one of them without caring which; everything that acts on "the
//! terminal" takes one.
//!
//! [`At`]: super::At

use super::*;

/// A tab name being edited.
pub(super) struct Renaming {
    pub(super) tab: usize,
    pub(super) draft: String,
    /// The second pane's name, when the tab is split. Two fields because the
    /// entry names two sessions; the `1:` and `2:` in front of them are the
    /// panes themselves and not part of either name.
    pub(super) second: Option<String>,
    /// Cleared after the field has been given focus once. Without this the
    /// terminal claims the keyboard back - it grabs focus whenever nothing else
    /// holds it - and the new name would be typed into IRIS.
    pub(super) focus: bool,
}

impl App {
    /// Connects a new tab to whatever `new_tab_profile` currently names.
    pub fn open_new_tab(&mut self) {
        let profile = self.new_tab_profile.clone();
        self.open_tab(profile);
    }

    pub fn open_tab(&mut self, profile: Profile) {
        let (cols, rows) = self.initial_size(&profile);
        self.tabs
            .push(Tab::new(profile, &self.settings, cols, rows));
        self.active = self.tabs.len() - 1;
    }

    /// Opens the easter egg, or goes back to the board that is already open.
    ///
    /// One at a time: typing `/snake` again is somebody coming back to the
    /// game rather than asking for a second one, and the score they were
    /// playing for is on the board they left.
    pub(super) fn open_snake_tab(&mut self) {
        if let Some(index) = self.tabs.iter().position(Tab::is_game) {
            self.active = index;
            return;
        }
        let game = Snake::new(Some(config::snake_score_path()));
        self.tabs.push(Tab::snake(game));
        self.active = self.tabs.len() - 1;
    }

    /// Closes a tab and everything in it - both sessions, when it is split.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // Ask IRIS to halt so it releases locks; Drop kills anything that
        // ignores the request.
        for session in self.tabs[index].sessions() {
            if let Some(session) = session.session.as_ref() {
                session.request_halt();
            }
        }
        let closed = self.tabs.remove(index);
        if !closed.is_game() {
            self.remember_closed(saved_tab(&closed));
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Closes one pane, and nothing else.
    ///
    /// On a tab that is not split there is only the one session, so this is
    /// [`App::close_tab`]. On a split tab it closes the session the user
    /// pointed at and leaves the other one in the tab, unsplit - which is what
    /// the right-click menu over a pane has to mean, since the other pane
    /// belongs to whatever is running in it.
    pub(super) fn close_pane(&mut self, at: At) {
        let Some(tab) = self.tabs.get_mut(at.tab) else {
            return;
        };
        if tab.split.is_none() {
            self.close_tab(at.tab);
            return;
        }
        // Ask IRIS to halt so it releases its locks; dropping the session kills
        // anything that ignores the request.
        let Some(going) = tab.pane(at.pane) else {
            return;
        };
        if let Some(session) = going.session.as_ref() {
            session.request_halt();
        }
        let closed = saved_pane(going);
        tab.close_pane(at.pane);
        self.remember_closed(closed);
    }

    fn remember_closed(&mut self, tab: SavedTab) {
        if self.closed_tabs.len() == CLOSED_TABS_KEPT {
            self.closed_tabs.remove(0);
        }
        self.closed_tabs.push(tab);
    }

    /// Opens the most recently closed tab again, in front: same profile, same
    /// name and split, back in the namespace it was in, or - for a shell that
    /// reports one - the folder. Each press goes one further back.
    pub(super) fn reopen_closed_tab(&mut self) {
        let Some(entry) = self.closed_tabs.pop() else {
            self.set_status(tr("No closed tab to reopen."));
            return;
        };
        self.restore_tab(entry);
        self.active = self.tabs.len() - 1;
    }

    /// The session the user is working in: the focused pane of the active tab.
    pub(super) fn active_tab(&self) -> Option<&Tab> {
        self.pane(self.focused_at())
    }

    /// Where that session lives.
    pub(super) fn focused_at(&self) -> At {
        At {
            tab: self.active,
            pane: self
                .tabs
                .get(self.active)
                .map_or(Pane::First, |tab| tab.focus),
        }
    }

    /// The session at `at`, while both the tab and the pane are still there.
    pub(super) fn pane(&self, at: At) -> Option<&Tab> {
        self.tabs.get(at.tab)?.pane(at.pane)
    }

    pub(super) fn pane_mut(&mut self, at: At) -> Option<&mut Tab> {
        self.tabs.get_mut(at.tab)?.pane_mut(at.pane)
    }

    /// Every session open, in every tab. What "is anything still connected"
    /// has to count, since a split tab holds two.
    pub(super) fn sessions(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().flat_map(|tab| tab.sessions())
    }

    /// The view of the pane the keyboard is in, which is where a find bar
    /// belongs: a search is made in one transcript, not in all of them.
    pub(super) fn focused_view(&self) -> Option<&terminal_view::ViewState> {
        let tab = self.tabs.get(self.active)?;
        Some(&tab.pane(tab.focus)?.view)
    }

    pub(super) fn focused_view_mut(&mut self) -> Option<&mut terminal_view::ViewState> {
        let tab = self.tabs.get_mut(self.active)?;
        let focus = tab.focus;
        Some(&mut tab.pane_mut(focus)?.view)
    }

    /// The tab strip, in no more than `width` of the row it is drawn in.
    ///
    /// Only ever called from a left-to-right row, and that matters:
    /// `allocate_ui_at_rect` gives the child the *parent's* layout, and what it
    /// then advances the row by is the child's `min_rect`. In a right-to-left
    /// parent that rect starts at the right-hand edge, so a narrow strip of
    /// tabs would measure as ending where the row ends and leave nothing after
    /// it - which is precisely how the title bar lost its drag area.
    pub(super) fn tab_strip_bounded(&mut self, ui: &mut egui::Ui, width: f32) {
        let height = ui.available_height().max(ui.spacing().interact_size.y);
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::Vec2::new(width, height));
        // The bar has already held back its own free strip to drag by - see
        // `FREE_STRIP` in `crate::ui::chrome` - and `width` is what is left,
        // across the theme's left and right spaces alike. Holding back more
        // here left a band of nothing between the tabs and the buttons.
        ui.allocate_ui_at_rect(rect, |ui| self.tab_strip_in(ui, 0.0, true));
    }

    /// The tab strip as a row of its own, the tabs sharing its width.
    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui) {
        self.tab_strip_in(ui, 0.0, false);
    }

    /// The tabs share the row out between them, the way GNOME's Files and
    /// Text Editor do - all of it but `reserve`, which in the title bar is
    /// the space the window is dragged by and must not be taken.
    ///
    /// `in_title_bar`: the tabs are what the window is moved by, so a tab
    /// dragged up or down moves it instead of reordering.
    fn tab_strip_in(&mut self, ui: &mut egui::Ui, reserve: f32, in_title_bar: bool) {
        let mut to_close = None;
        let mut to_rename = None;
        let mut to_split = None;
        let mut to_unsplit = None;
        let mut to_activate = None;
        let mut to_reopen = false;
        let can_reopen = !self.closed_tabs.is_empty();
        let with_namespace = self.settings.show_namespace_in_tab;
        let close_side = self.settings.tab_close_side;
        let theme = self.theme();
        // The shown tab is the terminal's own colour, so it runs on into the
        // terminal under it, as elementary's does; a theme that names its own
        // selected tab still gets that.
        let selected_colours = (
            Some(theme.tab_selected.unwrap_or(theme.background)),
            theme.tab_selected_text,
        );
        let divider = theme
            .ui_border
            .unwrap_or(theme.ui_foreground)
            .gamma_multiply(0.18);
        // Shrunk to the tabs rather than filling the row: in the title bar the
        // space left over is what the window is dragged by, and a scroll area
        // that claimed the whole width would take all of it.
        // The strip's own bar follows the same setting as the terminal's, so
        // "show scrollbars" means every scrollbar in the app rather than every
        // scrollbar except this one. Off still scrolls - the wheel and a drag
        // both work - it simply draws nothing along the bottom of the tabs.
        let visibility = if self.settings.show_scrollbars {
            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
        } else {
            egui::scroll_area::ScrollBarVisibility::AlwaysHidden
        };

        // The wheel over the strip, before the scroll area reads the input.
        //
        // egui turns a shifted wheel into horizontal scrolling for us, which
        // left the plain wheel - the one that is actually under the finger -
        // doing nothing at all over a row of tabs. The two are swapped back
        // here: the plain wheel scrolls the strip, and shift walks the
        // selection from tab to tab.
        //
        // Rewritten rather than handled afterwards so the scroll lands on the
        // same frame it was rolled on, and consumed when it changes tabs so
        // the strip does not also slide sideways under the pointer.
        let mut step = 0.0_f32;
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            ui.input_mut(|i| {
                if i.modifiers.shift {
                    // Already swapped by egui, so the shifted wheel arrives on
                    // x. `raw` rather than the smoothed delta: smoothing
                    // spreads one notch over several frames, which would walk
                    // the selection several tabs from one flick.
                    step = i.raw_scroll_delta.x;
                    i.raw_scroll_delta = egui::Vec2::ZERO;
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                } else {
                    i.raw_scroll_delta = egui::vec2(i.raw_scroll_delta.y, 0.0);
                    i.smooth_scroll_delta = egui::vec2(i.smooth_scroll_delta.y, 0.0);
                }
            });
        }
        if step != 0.0 && !self.tabs.is_empty() {
            // Up is back, the way it is in a list. Wrapped at both ends: the
            // wheel has no stop, and a selection that silently refuses to move
            // reads as the gesture not working.
            let last = self.tabs.len() - 1;
            to_activate = Some(if step > 0.0 {
                self.active.checked_sub(1).unwrap_or(last)
            } else if self.active >= last {
                0
            } else {
                self.active + 1
            });
        }

        // Where each tab was drawn this frame, and which one is being dragged:
        // what a drag is measured against once every tab has been laid out.
        let mut spans: Vec<(f32, f32)> = Vec::with_capacity(self.tabs.len());
        let mut dragging = None;
        // Set when a drag has been handed to the window, so the same drag is
        // not also read as reordering.
        let mut window_drag = false;
        let mut to_move = None;
        let gap = 0.0;
        let count = self.tabs.len().max(1) as f32;
        let room = ui.available_width() - reserve;
        // Fixed, each tab sizes itself to its name between the two bounds -
        // the space the tabs took before they shared the row - and is drawn
        // exactly as a shared one is.
        let shared = (self.settings.tab_width == crate::config::TabWidth::Shared)
            .then(|| ((room - gap * (count - 1.0)) / count).max(TAB_MIN_WIDTH));

        egui::ScrollArea::horizontal()
            .auto_shrink([true, false])
            .scroll_bar_visibility(visibility)
            .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let split = self.tabs[index].split.is_some();
                    let game = self.tabs[index].is_game();
                    let mut label = self.tabs[index].strip_label(with_namespace);
                    if self.tabs[index].focused().ended {
                        label.push_str(" (ended)");
                    } else if self.tabs[index].focused().sql {
                        // On the label rather than in the name, so renaming
                        // the tab neither loses it nor bakes it in.
                        label.push_str(" · SQL");
                    }
                    let (response, close) =
                        tab_pill(
                        ui,
                        self.tabs[index].uid,
                        selected,
                        label,
                        selected_colours,
                        shared,
                        close_side,
                    );
                    // A line between two tabs neither of which is shown: the
                    // shown one is set off by its colour already.
                    let next_selected = index + 1 == self.active;
                    if index + 1 < self.tabs.len() && !selected && !next_selected {
                        let r = response.rect;
                        ui.painter().line_segment(
                            [
                                egui::pos2(r.right() - 0.5, r.top() + r.height() * 0.25),
                                egui::pos2(r.right() - 0.5, r.bottom() - r.height() * 0.25),
                            ],
                            egui::Stroke::new(1.0_f32, divider),
                        );
                    }
                    let response =
                        response.on_hover_text(tr("Double-click to rename, drag to reorder."));
                    if response.clicked() {
                        to_activate = Some(index);
                    }
                    // The tab being moved is the one shown, the way it is
                    // everywhere else tabs are dragged: what is under the strip
                    // should be the session the user has hold of.
                    // In the title bar, a drag that sets off up or down is
                    // the window being moved - the bar has no other free
                    // space to take hold of - and so is any drag of a lone
                    // tab, which has nowhere to be reordered to.
                    if in_title_bar && response.drag_started() {
                        let moved = ui.input(|i| {
                            i.pointer
                                .press_origin()
                                .zip(i.pointer.interact_pos())
                                .map(|(from, to)| to - from)
                        });
                        let upright = moved.is_some_and(|d| d.y.abs() > d.x.abs());
                        if upright || self.tabs.len() == 1 {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            window_drag = true;
                        }
                    }
                    if response.drag_started() {
                        to_activate = Some(index);
                    }
                    if response.dragged() && !window_drag {
                        dragging = Some(index);
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    }
                    if response.double_clicked() {
                        to_rename = Some(index);
                    }
                    // What every other tabbed program does, and the reason the
                    // small cross is not the only way out: closing several
                    // tabs in a row means aiming at a cross the width of a
                    // character each time, and the middle button closes
                    // whatever is under the pointer.
                    if response.middle_clicked() {
                        to_close = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr("Rename...")).clicked() {
                            to_rename = Some(index);
                            ui.close_menu();
                        }
                        ui.separator();
                        // A tab already holding two sessions has nowhere to put
                        // a third, so the only thing offered is the way back.
                        if split {
                            if ui
                                .button(tr("Remove split"))
                                .on_hover_text(tr(
                                    "Gives the second session a tab of its own. Nothing is closed.",
                                ))
                                .clicked()
                            {
                                to_unsplit = Some(index);
                                ui.close_menu();
                            }
                        } else if !game {
                            if ui
                                .button(tr("Split to right"))
                                .on_hover_text(tr(
                                    "Opens a second session in this tab, beside this one. Click into a pane to type in it.",
                                ))
                                .clicked()
                            {
                                to_split = Some((index, SplitDir::Right));
                                ui.close_menu();
                            }
                            if ui.button(tr("Split to bottom")).clicked() {
                                to_split = Some((index, SplitDir::Bottom));
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui
                            .button(tr("Close"))
                            .on_hover_text(if split {
                                tr("Closes both sessions in this tab.")
                            } else {
                                tr("Closes this session.")
                            })
                            .clicked()
                        {
                            to_close = Some(index);
                            ui.close_menu();
                        }
                        // Here as well as on the keyboard, so the gesture can
                        // be found by anyone who does not know the chord.
                        if ui
                            .add_enabled(
                                can_reopen,
                                egui::Button::new(tr("Reopen closed tab"))
                                    .shortcut_text("Ctrl+Shift+T"),
                            )
                            .clicked()
                        {
                            to_reopen = true;
                            ui.close_menu();
                        }
                    });
                    if close.is_some_and(|close| close.on_hover_text(tr("Close this tab.")).clicked()) {
                        to_close = Some(index);
                    }
                    spans.push((response.rect.left(), response.rect.right()));
                }

                let pointer = ui.ctx().pointer_interact_pos();
                if let (Some(from), Some(pointer)) = (dragging, pointer) {
                    to_move = drop_slot(&spans, from, pointer.x).map(|to| (from, to));
                    // Held against either edge of the strip, the drag scrolls
                    // it: the tabs beyond the edge are where the user is
                    // trying to put this one, and they cannot be reached by
                    // a pointer that has nowhere further to go.
                    let visible = ui.clip_rect();
                    let edge = 24.0;
                    let push = if pointer.x < visible.left() + edge {
                        1.0
                    } else if pointer.x > visible.right() - edge {
                        -1.0
                    } else {
                        0.0
                    };
                    if push != 0.0 {
                        ui.scroll_with_delta(egui::vec2(push * 8.0, 0.0));
                        // The pointer holding still is still asking for the
                        // strip to move, and an idle loop asks for no frames.
                        ui.ctx().request_repaint();
                    }
                }
            });
        });

        if let Some((from, to)) = to_move {
            self.move_tab(from, to);
            // The tab ends where the pointer is, and it was active already
            // from the moment it was taken hold of.
            to_activate = Some(to);
        }
        if let Some(index) = to_activate {
            self.activate_tab(index);
        }
        if let Some((index, dir)) = to_split {
            self.split_tab(index, dir);
        }
        if let Some(index) = to_unsplit {
            self.remove_split(index);
        }
        if let Some(index) = to_rename {
            // Seeded with the names on screen, so renaming is an edit rather
            // than starting from nothing.
            let tab = &self.tabs[index];
            self.renaming = Some(Renaming {
                tab: index,
                draft: tab.title(with_namespace),
                second: tab
                    .pane(Pane::Second)
                    .map(|second| second.title(with_namespace)),
                focus: true,
            });
        }
        // Applied after the loop: closing a tab shifts every index after it.
        if let Some(index) = to_close {
            self.close_tab(index);
        }
        if to_reopen {
            self.reopen_closed_tab();
        }
    }

    /// Moves the tab at `from` to `to`, shifting the ones between along.
    ///
    /// Everything that remembers a tab by its index has to move with it: the
    /// active tab, and a rename in progress, which would otherwise commit the
    /// new name to whichever tab slid into the old place.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = moved_index(self.active, from, to);
        if let Some(renaming) = self.renaming.as_mut() {
            renaming.tab = moved_index(renaming.tab, from, to);
        }
    }

    /// Makes a tab the active one, which is the tab drawn and the one every
    /// other feature works on. Which of its panes is current is the tab's own
    /// business - see [`Tab::focus`].
    pub(super) fn activate_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// Splits a tab in two, opening a second session in the pane it makes.
    ///
    /// The new session goes where the pane appears - to the right, or at the
    /// bottom - and takes the keyboard, the way a newly opened tab does. A tab
    /// that is already split is left alone: it has two names to show and no
    /// room for a third.
    pub(super) fn split_tab(&mut self, index: usize, dir: SplitDir) {
        // A tab holding the easter egg holds no session and cannot hold one:
        // splitting it would put a live IRIS session in a pane nothing draws.
        if self
            .tabs
            .get(index)
            .is_none_or(|tab| tab.split.is_some() || tab.is_game())
        {
            return;
        }
        // The tab's own profile, not the one Ctrl+T would open: splitting a CMD
        // tab is asking for a second CMD beside it, and it used to come up as
        // an IRIS session because that was the new-tab default.
        let profile = self.tabs[index].profile.clone();
        // Down the middle to start with. The divider between them is what moves
        // it from there.
        self.open_split(index, dir, profile, 0.5);
        self.tabs[index].focus = Pane::Second;
        self.active = index;
    }

    /// Opens `profile` in a second pane of the tab at `index`, which the
    /// caller has made sure is not split already.
    fn open_split(&mut self, index: usize, dir: SplitDir, profile: Profile, ratio: f32) {
        let (cols, rows) = self.initial_size(&profile);
        let opened = Tab::new(profile, &self.settings, cols, rows);
        self.tabs[index].split = Some(Split {
            dir,
            tab: Box::new(opened),
            ratio,
        });
    }

    /// What [`App::restore_session`] needs to open these tabs again.
    ///
    /// The easter egg is left out: it has no far side to reconnect to, and
    /// `/snake` is how anybody who wants it back gets it.
    pub(super) fn session_snapshot(&self) -> SavedSession {
        let kept: Vec<bool> = self.tabs.iter().map(|tab| !tab.is_game()).collect();
        let tabs = self
            .tabs
            .iter()
            .filter(|tab| !tab.is_game())
            .map(saved_tab)
            .collect();
        SavedSession {
            active: kept_index(self.active, &kept),
            tabs,
        }
    }

    /// Opens again the tabs [`App::session_snapshot`] wrote down, in the same
    /// order, with the same names and splits, and the same one in front.
    ///
    /// Each one connects the way a newly opened tab would, autologon included:
    /// the profile is all it keeps, and the password was never in it.
    pub(super) fn restore_session(&mut self, saved: SavedSession) {
        for entry in saved.tabs {
            self.restore_tab(entry);
        }
        self.active = saved.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Opens one saved tab at the end of the strip.
    fn restore_tab(&mut self, entry: SavedTab) {
        self.open_tab(entry.profile);
        let index = self.tabs.len() - 1;
        self.tabs[index].custom_title = entry.title;
        self.tabs[index].replay(&entry.screen);
        self.tabs[index].resume_namespace = entry.namespace;
        if let Some(split) = entry.split {
            let dir = match split.dir {
                SavedDir::Right => SplitDir::Right,
                SavedDir::Bottom => SplitDir::Bottom,
            };
            // The file can be edited by hand, and a NaN would survive every
            // clamp the layout applies to it.
            let ratio = if split.ratio.is_finite() {
                split.ratio.clamp(0.0, 1.0)
            } else {
                0.5
            };
            self.open_split(index, dir, split.profile, ratio);
            if let Some(opened) = self.tabs[index].split.as_mut() {
                opened.tab.custom_title = split.title;
                opened.tab.replay(&split.screen);
                opened.tab.resume_namespace = split.namespace;
            }
            if entry.second_focused {
                self.tabs[index].focus = Pane::Second;
            }
        }
    }

    /// Takes a split tab back to one session, and gives the other one a tab of
    /// its own.
    ///
    /// Promoted rather than closed: it is a live IRIS session, and "remove
    /// split" is a sentence about the layout. Closing the tab is what closes
    /// both. The keyboard stays with whichever of the two had it.
    pub(super) fn remove_split(&mut self, index: usize) {
        let Some(split) = self.tabs.get_mut(index).and_then(|tab| tab.split.take()) else {
            return;
        };
        let had_focus = std::mem::take(&mut self.tabs[index].focus);
        self.tabs.insert(index + 1, *split.tab);
        self.active = if had_focus == Pane::Second {
            index + 1
        } else {
            index
        };
    }

    /// Names one tab.
    ///
    /// A window rather than an editable label in the strip: the terminal claims
    /// the keyboard whenever nothing else holds it, and a field that has to win
    /// that fight every frame is a worse trade than one dialog.
    pub(super) fn rename_tab_dialog(&mut self, ctx: &Context) {
        let Some(mut renaming) = self.renaming.take() else {
            return;
        };
        if renaming.tab >= self.tabs.len() {
            return;
        }

        let mut open = true;
        let mut commit = false;
        let mut cancel = false;

        crate::ui::dialog::show(ctx, "nit-rename-tab", tr("Rename tab"), &mut open, |ui| {
            {
                // A split tab is two sessions under one entry, so it is renamed
                // two names at a time. The `1:` and `2:` are the panes
                // themselves and cannot be edited away.
                let split = renaming.second.is_some();
                let field = |ui: &mut egui::Ui, label: Option<&str>, text: &mut String| {
                    let mut response = None;
                    ui.horizontal(|ui| {
                        if let Some(label) = label {
                            ui.label(label);
                        }
                        response =
                            Some(ui.add(egui::TextEdit::singleline(text).desired_width(220.0)));
                    });
                    response.expect("the field is always added")
                };

                let response = field(ui, split.then_some("1:"), &mut renaming.draft);
                if renaming.focus {
                    response.request_focus();
                    renaming.focus = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    commit = true;
                }
                if let Some(second) = renaming.second.as_mut() {
                    let response = field(ui, Some("2:"), second);
                    if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        commit = true;
                    }
                }

                // The one button there used to be for this is what an empty
                // field already does, and two ways to say the same thing in a
                // dialog this small only asks the user which one is real.
                ui.weak(tr("Empty goes back to default."));
                use crate::ui::dialog::{actions, button, Role};
                actions(ui, |ui| {
                    if button(ui, tr("Rename"), Role::Suggested).clicked() {
                        commit = true;
                    }
                    if button(ui, tr("Cancel"), Role::Plain).clicked() {
                        cancel = true;
                    }
                });
            }
        });

        if commit {
            let named = |text: &str| {
                let name = text.trim().to_string();
                (!name.is_empty()).then_some(name)
            };
            self.tabs[renaming.tab].custom_title = named(&renaming.draft);
            if let (Some(text), Some(second)) = (
                renaming.second.as_deref(),
                self.tabs[renaming.tab].pane_mut(Pane::Second),
            ) {
                second.custom_title = named(text);
            }
        } else if !(cancel || !open) {
            // Still open, so the draft survives to the next frame.
            self.renaming = Some(renaming);
        }
    }
}

/// The narrowest and widest a tab is drawn. Between the two it fits its name;
/// past the widest the name is cut short with an ellipsis, so one long name
/// cannot push every other tab off the strip.
const TAB_MIN_WIDTH: f32 = 120.0;
const TAB_MAX_WIDTH: f32 = 220.0;

/// The air between a tab's close button and its name. The name is held off
/// both ends by this much, not only the button's, so it stays centred whichever
/// side the button is on; without it a name long enough to be cut short ran
/// right up against the cross.
const TAB_CLOSE_GAP: f32 = 6.0;

/// How tall a tab is: a little taller than a button, so the strip reads as
/// tabs rather than as one more row of buttons. The title bar sizes its row by
/// this when the tabs are in it, or the buttons ahead of them would be centred
/// on a row the tabs then make taller.
pub(super) fn tab_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y + 4.0
}

/// One tab, drawn the way elementary's Files and Terminal draw theirs: the
/// whole height of its bar, square, the one shown filled with the terminal's
/// colour so it runs on into the terminal, the rest flat until hovered. The
/// close button is inside it, at whichever end `close_at` says - shown on the
/// tab being looked at and on the one under the pointer - and the name is
/// centred. `width` is
/// the tab's share of a row it fills; `None` sizes it to its name.
///
/// The close button is registered after the tab, so it is the one a click on
/// it reaches; the tab itself takes clicks, drags and the context menu.
fn tab_pill(
    ui: &mut egui::Ui,
    uid: u64,
    selected: bool,
    text: String,
    colours: (Option<egui::Color32>, Option<egui::Color32>),
    width: Option<f32>,
    close_at: crate::config::TabCloseSide,
) -> (egui::Response, Option<egui::Response>) {
    let base = tab_height(ui);
    let height = ui.available_height().max(base);
    let close_side = base - 8.0;
    let pad = 8.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let outer = width.unwrap_or(TAB_MAX_WIDTH);
    let inset = pad + close_side + TAB_CLOSE_GAP;
    let room = outer - 2.0 * inset;
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, egui::Color32::PLACEHOLDER);
    job.wrap = egui::text::TextWrapping {
        max_width: room,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.fonts(|f| f.layout_job(job));
    let width = width
        .unwrap_or_else(|| (galley.size().x + 2.0 * inset).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("nit-tab", uid)),
        egui::Sense::click_and_drag(),
    );
    let close_x = match close_at {
        crate::config::TabCloseSide::Left => rect.left() + pad + close_side / 2.0,
        crate::config::TabCloseSide::Right => rect.right() - pad - close_side / 2.0,
    };
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(close_x, rect.center().y),
        egui::Vec2::splat(close_side),
    );
    let shows_close = selected || ui.rect_contains_pointer(rect);
    let close = shows_close.then(|| {
        ui.interact(
            close_rect,
            egui::Id::new(("nit-tab-close", uid)),
            egui::Sense::click(),
        )
    });

    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        if selected {
            let fill = colours.0.unwrap_or(visuals.extreme_bg_color);
            ui.painter().rect_filled(rect, 0.0, fill);
        } else if response.hovered() || response.dragged() {
            ui.painter().rect_filled(
                rect,
                0.0,
                visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.5),
            );
        }
        let ink = match colours.1 {
            Some(ink) if selected => ink,
            _ if selected => visuals.strong_text_color(),
            _ => visuals.text_color().gamma_multiply(0.7),
        };
        let at = rect.center() - galley.size() / 2.0;
        ui.painter().galley(at, galley, ink);

        if let Some(close) = close.as_ref() {
            if close.hovered() {
                ui.painter()
                    .rect_filled(close_rect, 2.0, visuals.widgets.hovered.bg_fill);
            }
            let arm = close_side * 0.22;
            let c = close_rect.center();
            let stroke = egui::Stroke::new(1.4_f32, ink);
            ui.painter().line_segment(
                [c + egui::vec2(-arm, -arm), c + egui::vec2(arm, arm)],
                stroke,
            );
            ui.painter().line_segment(
                [c + egui::vec2(-arm, arm), c + egui::vec2(arm, -arm)],
                stroke,
            );
        }
    }
    (response, close)
}

/// Where the tab being dragged belongs, given the left and right edge of every
/// tab as drawn and where the pointer is - or `None` while it is still over its
/// own place.
///
/// A tab moves once the pointer passes the *middle* of a neighbour, not its
/// edge. Tabs differ in width, and swapping at the edge would put a wide
/// neighbour straight back under the pointer, which would swap them back on
/// the next frame and flicker for as long as the pointer stayed there. Past
/// the middle, the neighbour ends up on the far side of the pointer either way.
///
/// A pointer that has passed several middles in one frame - a fast flick -
/// moves the tab all the way, not one place per frame.
fn drop_slot(spans: &[(f32, f32)], from: usize, x: f32) -> Option<usize> {
    let middle = |i: usize| (spans[i].0 + spans[i].1) / 2.0;
    if from >= spans.len() {
        return None;
    }
    (from + 1..spans.len())
        .rev()
        .find(|&i| x > middle(i))
        .or_else(|| (0..from).find(|&i| x < middle(i)))
}

/// Where the tab at `index` is after the one at `from` has moved to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && (from + 1..=to).contains(&index) {
        index - 1
    } else if to < from && (to..from).contains(&index) {
        index + 1
    } else {
        index
    }
}

/// How many closed tabs Ctrl+Alt+T can go back through. Each one holds up to
/// [`SAVED_SCREEN_LINES`] of text, so the list is not left to grow all day.
const CLOSED_TABS_KEPT: usize = 20;

/// A tab as [`SavedTab`] keeps it, split included.
fn saved_tab(tab: &Tab) -> SavedTab {
    SavedTab {
        split: tab.split.as_ref().map(|split| SavedSplit {
            dir: match split.dir {
                SplitDir::Right => SavedDir::Right,
                SplitDir::Bottom => SavedDir::Bottom,
            },
            ratio: split.ratio,
            profile: saved_profile(&split.tab),
            namespace: saved_namespace(&split.tab),
            title: split.tab.custom_title.clone(),
            screen: saved_screen(&split.tab),
        }),
        second_focused: tab.split.is_some() && tab.focus == Pane::Second,
        ..saved_pane(tab)
    }
}

/// One pane on its own, as a tab of its own: what closing one side of a split
/// leaves to reopen.
fn saved_pane(tab: &Tab) -> SavedTab {
    SavedTab {
        profile: saved_profile(tab),
        namespace: saved_namespace(tab),
        title: tab.custom_title.clone(),
        screen: saved_screen(tab),
        ..SavedTab::default()
    }
}

/// How many lines of a pane's output are kept for the next run. Enough to see
/// what was being done in it; the transcript log is where the whole of it is.
const SAVED_SCREEN_LINES: usize = 2_000;

/// A pane's profile, with a shell pointed at the folder it was last in.
///
/// Only a folder that is still there: a shell asked to start somewhere that
/// has gone does not start at all, and where it opened is a better answer.
fn saved_profile(tab: &Tab) -> Profile {
    let mut profile = tab.profile.clone();
    if let (Some(shell), Some(cwd)) = (profile.shell.as_mut(), tab.cwd.as_ref()) {
        let cwd = std::path::PathBuf::from(cwd);
        if cwd.is_dir() {
            shell.cwd = Some(cwd);
        }
    }
    profile
}

/// The namespace to take a pane back to. Only an IRIS one has any, and a pane
/// that never reached a prompt has nothing worth going back to.
fn saved_namespace(tab: &Tab) -> Option<String> {
    tab.namespace.clone().filter(|_| !tab.profile.is_shell())
}

/// The last [`SAVED_SCREEN_LINES`] lines a pane showed, as plain text.
fn saved_screen(tab: &Tab) -> String {
    let text = analyze::transcript(&tab.grid, analyze::Scope::All);
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(SAVED_SCREEN_LINES);
    lines[from..].join("\n")
}

/// Where the tab at `index` ends up once only the tabs marked in `kept` are
/// left: the nearest kept tab before it when it was not kept, or the first
/// one when there is none before it.
fn kept_index(index: usize, kept: &[bool]) -> usize {
    let before = kept.iter().take(index).filter(|&&k| k).count();
    let survives = kept.get(index).copied().unwrap_or(false);
    if survives {
        before
    } else {
        before.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_tab_keeps_its_place_when_a_game_before_it_is_left_out() {
        assert_eq!(kept_index(2, &[true, false, true]), 1);
        assert_eq!(kept_index(0, &[true, false, true]), 0);
    }

    #[test]
    fn an_active_game_that_is_left_out_hands_the_front_to_the_tab_before_it() {
        assert_eq!(kept_index(1, &[true, false, true]), 0);
        assert_eq!(kept_index(0, &[false, true]), 0);
        assert_eq!(kept_index(0, &[false]), 0);
    }

    /// Three tabs of different widths, side by side: 0..50, 50..250, 250..300.
    const SPANS: [(f32, f32); 3] = [(0.0, 50.0), (50.0, 250.0), (250.0, 300.0)];

    #[test]
    fn a_tab_stays_put_until_the_pointer_passes_a_neighbours_middle() {
        assert_eq!(drop_slot(&SPANS, 0, 140.0), None, "short of 150");
        assert_eq!(drop_slot(&SPANS, 0, 160.0), Some(1));
        assert_eq!(
            drop_slot(&SPANS, 2, 160.0),
            None,
            "short of 150, from the right"
        );
        assert_eq!(drop_slot(&SPANS, 2, 140.0), Some(1));
    }

    /// Swapped past the middle, the wide neighbour lands on the far side of
    /// the pointer - so the next frame, with the pointer where it was, leaves
    /// the tabs alone rather than swapping them back.
    #[test]
    fn a_swap_with_a_wider_neighbour_does_not_swap_straight_back() {
        let x = 160.0;
        assert_eq!(drop_slot(&SPANS, 0, x), Some(1));
        // After the swap: the wide tab first, the dragged one after it.
        let swapped = [(0.0, 200.0), (200.0, 250.0), (250.0, 300.0)];
        assert_eq!(drop_slot(&swapped, 1, x), None);
    }

    #[test]
    fn a_fast_drag_moves_the_tab_past_every_middle_it_crossed() {
        assert_eq!(drop_slot(&SPANS, 0, 290.0), Some(2));
        assert_eq!(drop_slot(&SPANS, 2, 10.0), Some(0));
    }

    #[test]
    fn the_pointer_over_the_dragged_tab_itself_moves_nothing() {
        assert_eq!(drop_slot(&SPANS, 1, 60.0), None);
        assert_eq!(drop_slot(&SPANS, 1, 240.0), None);
    }

    /// Every index is carried along: the moved tab goes to its new place, the
    /// ones it passed shift by one towards where it came from, and the rest
    /// stay where they were.
    #[test]
    fn moving_a_tab_shifts_only_the_tabs_it_passed() {
        // 0 1 2 3 4, with 1 moved to 3: 0 2 3 1 4.
        let after: Vec<usize> = (0..5).map(|i| moved_index(i, 1, 3)).collect();
        assert_eq!(after, vec![0, 3, 1, 2, 4]);
        // And back the other way, 3 to 1: 0 3 1 2 4.
        let after: Vec<usize> = (0..5).map(|i| moved_index(i, 3, 1)).collect();
        assert_eq!(after, vec![0, 2, 3, 1, 4]);
    }

    /// The same remapping, checked against what `Vec::remove` and `insert`
    /// actually do to the tabs, for every pair of places.
    #[test]
    fn the_remapping_agrees_with_moving_the_tabs_themselves() {
        for from in 0..5 {
            for to in 0..5 {
                let mut tabs: Vec<usize> = (0..5).collect();
                let tab = tabs.remove(from);
                tabs.insert(to, tab);
                for original in 0..5 {
                    let now = moved_index(original, from, to);
                    assert_eq!(tabs[now], original, "{original}, moving {from} to {to}");
                }
            }
        }
    }
}
