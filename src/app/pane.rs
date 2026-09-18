//! Drawing one terminal pane, and the panel it lives in.
//!
//! The frame's hot path: everything here runs for every pane, every frame, so
//! the cost of what it does is the cost of the app. See
//! [`crate::ui::terminal_view`] for the grid itself.

use super::*;

impl App {
    /// How far the terminal is held back from the window's own edges.
    ///
    /// The resize grips live in a foreground layer along each edge, which
    /// outranks whatever is under them for hit-testing - and the terminal
    /// reaches three of those edges. Its first column was therefore inside the
    /// left-hand grip: the pointer turned into a resize arrow over it, and a
    /// drag starting there resized the window instead of selecting the text.
    /// Holding the grid back by exactly the width of the grip gives the column
    /// back without taking anything away from the grip.
    ///
    /// A point wider than the grip rather than exactly as wide: at exactly as
    /// wide, the first column began on the last coordinate the grip still
    /// answered to, which is the boundary case that left this reported as
    /// unfixed. The grip is also told to keep off the panes outright - see
    /// `keep_out` in [`chrome::resize_grips`] - so the two now disagree in the
    /// terminal's favour whatever the arithmetic works out to.
    ///
    /// Nothing is held back at the top - the tab strip is there, not the
    /// terminal.
    pub(super) fn terminal_inset(&self) -> egui::Margin {
        let gutter = chrome::RESIZE_GRAB + 1.0;
        egui::Margin {
            left: gutter,
            right: gutter,
            top: 0.0,
            bottom: gutter,
        }
    }

    /// Draws one pane - a tab's own notes and its terminal - and carries out
    /// everything the mouse and the keyboard did in it.
    ///
    /// `focused` marks the pane the keyboard is in - the focused pane of the
    /// active tab, and the only one drawn when a tab is not split. It is the
    /// one that takes the keyboard back when no other widget wants it; the
    /// other pane takes it when it is clicked into, which is also what moves
    /// the tab's focus there.
    pub(super) fn terminal_pane(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        theme: &Theme,
        at: At,
        focused: bool,
    ) -> PaneMeasure {
        let mut opts = self.render_opts();
        // ObjectScript colouring on a `bash` session colours the wrong things:
        // a `$` is a variable to the highlighter and a prompt to the shell.
        // Turned off for the pane rather than for the app, so an IRIS tab
        // beside a shell tab keeps its colours.
        if self.pane(at).is_some_and(|tab| tab.profile.is_shell()) {
            opts.syntax = false;
            // A shell draws to the width it is told, so telling it the grid's
            // full margin makes every full-screen program lay itself out
            // that wide and then wrap into screenfuls of padding. Only IRIS,
            // which truncates instead of drawing, needs the wide margin.
            opts.wide_grid = false;
        }
        let uid = match self.pane(at) {
            Some(tab) => tab.uid,
            // Nothing there to draw: a tab that went away between frames.
            None => return PaneMeasure::default(),
        };
        // Only the focused pane follows the active session, and only it claims
        // the keyboard when nothing else holds it. Two panes both claiming it
        // would take it from each other every frame.
        let role = terminal_view::PaneRole {
            focused,
            take_focus: focused && self.focused_tab != Some(uid),
            split: self.tabs.get(at.tab).is_some_and(|tab| tab.split.is_some()),
        };
        if focused {
            self.focused_tab = Some(uid);
        }

        // The macro list is lent to the pane for the length of the draw, and
        // taken back straight afterwards. Moved out rather than borrowed
        // because drawing a pane borrows the whole app mutably to reach the
        // tab, and the menu is drawn from inside that - the list itself is
        // never written from there.
        let groups = std::mem::take(&mut self.macro_groups);
        let (mut result, has_selection, tooltip) = {
            let Some(tab) = self.pane_mut(at) else {
                self.macro_groups = groups;
                return PaneMeasure::default();
            };
            let has_selection = tab.view.selection.map(|s| !s.is_empty()).unwrap_or(false);
            let result = draw_pane(ui, tab, theme, &opts, role, &groups);
            // Asked for here rather than where the hover was found: `request`
            // needs the tab's session and namespace, which the terminal view
            // has no reason to know about.
            let tooltip = result.piece_hover.clone().map(|piece| {
                let namespace = tab.namespace.clone().unwrap_or_default();
                let lookup = tab.doc_lookup.request(&namespace, &piece.global);
                (piece, lookup)
            });
            (result, has_selection, tooltip)
        };
        self.macro_groups = groups;
        if let Some((piece, lookup)) = tooltip {
            // A pending lookup is answered by a session of this pane's own,
            // whose reader wakes the loop when it says something - but the
            // steps in between (opening it, noticing it has reached a prompt)
            // have nothing to schedule them, and an idle terminal asks for no
            // frames at all. This is the only place that knows one is
            // outstanding.
            if lookup == Lookup::Pending {
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            // Always something to say: the piece number is counted off the row
            // itself and does not depend on IRIS having heard of the global.
            result.response = result
                .response
                .on_hover_ui_at_pointer(|ui| piece_tooltip(ui, &piece, &lookup));
        }

        // This pane's own session, sized to this pane: a pane is a window onto
        // one session, and telling IRIS about the whole split area would
        // truncate its output at a width nothing is drawn at.
        if !self.minimized {
            if let Some(tab) = self.pane_mut(at) {
                tab.resize(result.cols, result.rows);
            }
        }
        self.pane_rects.push(result.response.rect);
        let measure = PaneMeasure {
            grid: (result.cols, result.rows),
            view: (result.view_cols, result.view_rows),
            cell: result.cell,
            clicked: result.response.clicked(),
        };

        // Right-click menu actions reuse the same paths as the keyboard
        // shortcuts. Copy-on-select: a finished drag goes straight to the
        // clipboard, without waiting for Ctrl+C.
        if result.copy_selection {
            self.copy_selection(ctx, at);
        }
        if let Some(columns) = result.cursor_move {
            self.move_cursor(at, columns);
        }

        if let Some(action) = result.context_action {
            use terminal_view::ContextAction;
            match action {
                ContextAction::CopySelection => self.copy_selection(ctx, at),
                ContextAction::CopyAndPaste => self.copy_and_paste(ctx, at),
                ContextAction::Paste => {
                    // The clipboard is only readable through egui's paste
                    // event, so ask for one rather than reaching for the OS
                    // clipboard behind egui's back.
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
                ContextAction::SelectAll => {
                    if let Some(tab) = self.pane_mut(at) {
                        let grid = &tab.grid;
                        tab.view.select_all(grid);
                    }
                }
                ContextAction::ClearSelection => {
                    if let Some(tab) = self.pane_mut(at) {
                        tab.view.clear_selection();
                    }
                }
                ContextAction::Export(format, range) => self.export(range, format),
                ContextAction::CopyRange(range) => {
                    self.handle_request(ctx, UiRequest::CopyRange(range));
                }
                // Both go through the ordinary request path, which is what
                // decides between sending at once and opening the dialog that
                // asks for the parameters or the confirmation first.
                ContextAction::RunMacro(m) => self.handle_request(ctx, UiRequest::RunMacro(m)),
                ContextAction::RunNative(native) => {
                    self.panels.pending_native = Some(panels::PendingNative::new(native));
                }
                ContextAction::Analyze(scope, panes) => self.analyze_with_claude(at, scope, panes),
                ContextAction::ClearTerminal => self.clear_terminal(at),
                // The layout of the tab this pane is in. Recorded rather than
                // done, because the tabs are being drawn - see
                // [`LayoutAction`]. `at` is the pane the menu was opened over,
                // so Close closes that session and nothing else.
                ContextAction::SplitRight => {
                    self.pending_layout = Some(LayoutAction::Split(at.tab, SplitDir::Right));
                }
                ContextAction::SplitBottom => {
                    self.pending_layout = Some(LayoutAction::Split(at.tab, SplitDir::Bottom));
                }
                ContextAction::Unsplit => {
                    self.pending_layout = Some(LayoutAction::Unsplit(at.tab));
                }
                ContextAction::ClosePane => {
                    self.pending_layout = Some(LayoutAction::ClosePane(at));
                }
            }
        }

        // Only the focused terminal consumes keystrokes.
        if result.response.clicked() {
            result.response.request_focus();
        }
        // While the macro editor is listening for a chord, the keys belong to
        // the shortcut being recorded. The editor is drawn after this and takes
        // them out of the frame's events itself, but that is too late to stop
        // them reaching IRIS from here - and a recorded Ctrl+D would otherwise
        // have ended the session it was recorded in.
        //
        // And nothing is typed into IRIS while this window is not the active
        // one. That should never arise - the system sends keys to the window
        // that has them - but the shell puts surfaces of its own over the top
        // of whatever is in front, the clipboard history (Win+V) among them,
        // and a keystroke meant for one of those must not reach a live session:
        // Up and Down at an IRIS prompt walk the command history, and the
        // session is on a database the whole team shares. `focused` is unknown
        // on a platform that does not report it, and unknown counts as focused
        // rather than leaving the terminal unable to type at all.
        let window_active = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        // Either of the two shortcut pickers - a macro's binding, or the macro
        // manager's own chord - is listening, and the keys belong to whichever
        // one it is.
        let capturing = self.panels.macros.capture_shortcut || self.panels.capture_manager_shortcut;
        if !result.response.has_focus() || capturing || !window_active {
            return measure;
        }

        let events = ui.input(|i| i.events.clone());
        let (insert_down, delete_down) = input::chord_keys_down();
        let (input_ctx, encoding) = {
            let Some(tab) = self.pane(at) else {
                return measure;
            };
            let line = lineedit::current(&tab.grid);
            let input_ctx = input::InputContext {
                has_selection,
                line,
                app_cursor_keys: tab.grid.app_cursor_keys,
                shell: tab.profile.shell.is_some(),
                recall_mid_line: self.settings.recall_mid_line,
                // This session's own commands, or anything an earlier run
                // left behind. Not what another tab has typed since.
                can_recall: !tab.commands.is_empty() || !self.history.earlier().is_empty(),
                selected_span: line.and_then(|line| selection_in_line(tab, line)),
                insert_down,
                delete_down,
            };
            (input_ctx, tab.profile.wire_encoding())
        };
        let mut action = input::translate(&events, &input_ctx);

        if action.select_line {
            self.select_typed_line(at);
        }
        if let Some(motion) = action.extend_selection {
            self.extend_selection(at, motion);
        }
        if let Some(motion) = action.move_cursor {
            self.move_by(at, motion);
        }
        if action.collapse_selection {
            if let Some(tab) = self.pane_mut(at) {
                tab.view.clear_selection();
            }
        }
        // A quote or an opening bracket typed over a selection wraps it rather
        // than replacing it, the way an editor does - so picking a global name
        // off the screen and quoting it is one keystroke. Only a single
        // character typed on its own: a paste is a replacement whatever it
        // happens to start with.
        let surrounded = self.settings.surround_selection
            && input_ctx.selected_span.is_some()
            && !action.select_line
            && action.paste.is_none()
            && one_char(&action.text)
                .and_then(input::surround_pair)
                .is_some()
            && self.surround_selection(at, one_char(&action.text).unwrap_or(' '));
        if surrounded {
            // The line has been rewritten and the selection moved with it; the
            // character that asked for it must not also be typed.
            action.text.clear();
        }

        // A selection inside the command line otherwise behaves the way one in
        // a text field does: an erase key takes it out, and so does typing or
        // pasting over it, before the new text goes in behind it.
        let replaced = !surrounded && input_ctx.selected_span.is_some() && !action.select_line && {
            !action.text.is_empty() || action.paste.is_some()
        };
        if action.erase_selection || replaced {
            self.erase_selection(at);
        }

        // Recorded before the Enter reaches IRIS, while the line is still on
        // screen to be read.
        if let Some(unechoed) = action.submitted.as_deref() {
            // Submitting ends the selection with the line it was on, rather
            // than leaving it highlighted in the scrollback.
            if let Some(tab) = self.pane_mut(at) {
                tab.view.clear_selection();
            }
            self.record_command(at, unechoed);
        }
        if let Some(direction) = action.recall {
            self.recall(at, direction);
        }
        // Typing abandons wherever the recall had walked to.
        if !action.text.is_empty() {
            if let Some(tab) = self.pane_mut(at) {
                tab.recall_step = None;
            }
        }

        if action.copy {
            self.copy_selection(ctx, at);
        }
        if let Some(text) = action.paste.take() {
            let text = input::sanitize_paste(&text);
            let encoded = self.plugins.on_input(&encoding.encode(&text));
            // While the prompt line is still on screen to be read from.
            self.record_pasted(at, &text);
            if let Some(tab) = self.pane_mut(at) {
                // A paste rewrites the line, so wherever the recall had walked
                // to is no longer where the line came from.
                tab.recall_step = None;
                tab.send(&encoded);
            }
        }
        // IRIS never reports its insert/replace state, so the keystroke is the
        // only signal there is. Display only - see `Grid::insert_mode`.
        if action.toggle_insert {
            if let Some(tab) = self.pane_mut(at) {
                tab.grid.insert_mode = !tab.grid.insert_mode;
            }
        }
        if !action.is_empty() {
            let mut wire = encoding.encode(&action.text);
            wire.extend_from_slice(&action.bytes);
            let wire = self.plugins.on_input(&wire);
            if let Some(tab) = self.pane_mut(at) {
                // Typing always returns the view to the live output.
                tab.view.scroll_to_bottom();
                tab.send(&wire);
            }
        }

        measure
    }

    /// [`App::terminal_pane`] in a box of a given size, for one half of a
    /// split.
    pub(super) fn sized_pane(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        theme: &Theme,
        at: At,
        focused: bool,
        size: egui::Vec2,
    ) -> PaneMeasure {
        ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(size);
            self.terminal_pane(ui, ctx, theme, at, focused)
        })
        .inner
    }
}

/// The piece tooltip's contents.
///
/// The piece number is always shown: it is counted off the row, and holds
/// whether or not IRIS can say anything about the global. Everything else -
/// the description, size, type and formatted value - needs a class that maps
/// this global at this subscript shape, and plenty of globals have none.
/// An undocumented global, one no map claims, and a piece a map leaves out
/// all come out the same way, as the bare number, because from where the user
/// is standing they are the same thing: nobody wrote this down.
fn piece_tooltip(ui: &mut egui::Ui, piece: &terminal_view::PieceSelection, lookup: &Lookup) {
    // A tooltip sizes itself to its contents, and a described piece's are one
    // long line of prose per row, which egui left to itself wraps into a
    // column a few characters wide. The minimum is the width that fits a
    // description without shredding it and is set only where there is one -
    // a bare `Piece: 1` in a 240px box is just an empty box. Anything longer
    // than the maximum still wraps, which is what the maximum is for.
    ui.set_max_width(460.0);

    match lookup {
        Lookup::Pending => {
            ui.label(tr1("Looking up ^{}…", &piece.global));
        }
        Lookup::Ready(maps) => {
            let Some(found) = describe_hover(maps, piece) else {
                ui.label(tr1("Piece: {}", &piece.piece.to_string()));
                return;
            };
            ui.set_min_width(240.0);
            ui.label(tr2(
                "Piece: {} - {}",
                &found.info.label(),
                &found.info.description,
            ));
            if !found.info.size.is_empty() {
                ui.label(tr1("Size: {}", &found.info.size));
            }
            if !found.info.kind.is_empty() {
                ui.label(tr1("Type: {}", &found.info.kind));
            }
            match doc_lookup::format_value(found.info, &found.raw) {
                doc_lookup::Formatted::Value(formatted) => {
                    ui.label(tr1("Formatted value: {}", &formatted));
                }
                doc_lookup::Formatted::Invalid => {
                    ui.label(tr1("Formatted value: {}", tr("invalid value")));
                }
                doc_lookup::Formatted::Nothing => {}
            }
        }
        Lookup::NotFound | Lookup::Unavailable => {
            ui.label(tr1("Piece: {}", &piece.piece.to_string()));
        }
    }
}

/// The hovered piece against the map whose subscript shape this row has.
///
/// Split out because both the tooltip and the decision of whether to show one
/// at all need the same answer, and asking twice for it is the only way to
/// keep that decision out of the drawing code.
fn describe_hover<'a>(
    maps: &'a [doc_lookup::MapInfo],
    piece: &terminal_view::PieceSelection,
) -> Option<doc_lookup::Described<'a>> {
    doc_lookup::describe(
        maps,
        &piece.subscripts,
        piece.piece,
        &piece.piece_text,
        piece.offset,
    )
}
