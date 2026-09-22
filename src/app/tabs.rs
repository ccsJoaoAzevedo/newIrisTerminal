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
        self.tabs.remove(index);
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
        if let Some(going) = tab.pane(at.pane).and_then(|pane| pane.session.as_ref()) {
            going.request_halt();
        }
        tab.close_pane(at.pane);
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
        ui.allocate_ui_at_rect(rect, |ui| self.tab_strip(ui));
    }

    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        let mut to_rename = None;
        let mut to_split = None;
        let mut to_unsplit = None;
        let mut to_activate = None;
        let with_namespace = self.settings.show_namespace_in_tab;
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
        let mut to_move = None;

        egui::ScrollArea::horizontal()
            .auto_shrink([true, false])
            .scroll_bar_visibility(visibility)
            .show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let split = self.tabs[index].split.is_some();
                    let game = self.tabs[index].is_game();
                    let mut label = self.tabs[index].strip_label(with_namespace);
                    if self.tabs[index].focused().ended {
                        label.push_str(" (ended)");
                    }
                    let response = tab_label(ui, self.tabs[index].uid, selected, label)
                        .on_hover_text(tr("Double-click to rename, drag to reorder."));
                    if response.clicked() {
                        to_activate = Some(index);
                    }
                    // The tab being moved is the one shown, the way it is
                    // everywhere else tabs are dragged: what is under the strip
                    // should be the session the user has hold of.
                    if response.drag_started() {
                        to_activate = Some(index);
                    }
                    if response.dragged() {
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
                    });
                    let close = icon_button(ui, icons::Glyph::SmallCross, None)
                        .on_hover_text(tr("Close this tab."));
                    if close.clicked() {
                        to_close = Some(index);
                    }
                    spans.push((response.rect.left(), close.rect.right()));
                    ui.separator();
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
        let (cols, rows) = self.initial_size(&self.new_tab_profile.clone());
        let opened = Tab::new(self.new_tab_profile.clone(), &self.settings, cols, rows);
        self.tabs[index].split = Some(Split {
            dir,
            tab: Box::new(opened),
            // Down the middle to start with. The divider between them is what
            // moves it from there.
            ratio: 0.5,
        });
        self.tabs[index].focus = Pane::Second;
        self.active = index;
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

        egui::Window::new(tr("Rename tab"))
            .id(egui::Id::new("nit-rename-tab"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
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
                ui.small(tr("Empty goes back to default."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Rename")).clicked() {
                        commit = true;
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
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

/// One tab's name in the strip: a selectable label, drawn exactly as egui's
/// own, that can also be dragged.
///
/// Not `ui.selectable_label`, because that takes its id from egui's counter,
/// which numbers widgets by where they are drawn - and `push_id` around it
/// does not help, since a child `Ui` seeds its counter from its parent's
/// rather than from the id it was given. A drag is tracked by id, and the tab
/// being dragged changes place every time it passes another one: with an id
/// that belongs to the place, the drag stayed behind and was taken up by
/// whichever tab moved into it, which then moved in turn, and the tabs went
/// round in a circle under a pointer that was holding still. The id here is
/// the tab's own, so it goes wherever the tab goes.
///
/// Click and drag both: egui only calls it a drag once the pointer has moved
/// past the click threshold, so a click still selects and a double-click still
/// renames.
fn tab_label(ui: &mut egui::Ui, uid: u64, selected: bool, text: String) -> egui::Response {
    let padding = ui.spacing().button_padding;
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        None,
        ui.available_width() - 2.0 * padding.x,
        egui::TextStyle::Button,
    );
    let mut size = galley.size() + 2.0 * padding;
    size.y = size.y.max(ui.spacing().interact_size.y);
    let (rect, _) = ui.allocate_at_least(size, egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("nit-tab", uid)),
        egui::Sense::click_and_drag(),
    );

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.highlighted() || response.has_focus() {
            ui.painter().rect(
                rect.expand(visuals.expansion),
                visuals.rounding,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
            );
        }
        let at = ui
            .layout()
            .align_size_within_rect(galley.size(), rect.shrink2(padding))
            .min;
        ui.painter().galley(at, galley, visuals.text_color());
    }
    response
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

#[cfg(test)]
mod tests {
    use super::*;

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
