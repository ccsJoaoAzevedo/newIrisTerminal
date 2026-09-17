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

        egui::ScrollArea::horizontal()
            .auto_shrink([true, false])
            .scroll_bar_visibility(visibility)
            .show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let split = self.tabs[index].split.is_some();
                    let mut label = self.tabs[index].strip_label(with_namespace);
                    if self.tabs[index].focused().ended {
                        label.push_str(" (ended)");
                    }
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(tr("Double-click to rename."));
                    if response.clicked() {
                        to_activate = Some(index);
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
                        } else {
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
                    if icon_button(ui, icons::Glyph::SmallCross, None)
                        .on_hover_text(tr("Close this tab."))
                        .clicked()
                    {
                        to_close = Some(index);
                    }
                    ui.separator();
                }
            });
        });

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
        if self.tabs.get(index).is_none_or(|tab| tab.split.is_some()) {
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
