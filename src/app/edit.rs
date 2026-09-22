//! Acting on the line the user is typing, and on what is selected.
//!
//! None of this edits a buffer of its own. The far side owns the line being
//! typed and the cursor in it, so every one of these turns an intent into the
//! keystrokes that would have produced it and sends those - which is why they
//! all end in a `send`, and why they all have to know where the cursor is.

use super::*;

impl App {
    /// Clears the active terminal and forgets its history.
    ///
    /// The deliberate version of what `W #` only looks like: IRIS's own
    /// clear-screen files the screen into the scrollback, and this is the
    /// gesture for when the history really is meant to go. Also on the
    /// right-click menu, because a shortcut nobody can see is a shortcut nobody
    /// uses.
    ///
    /// Clearing the grid on its own is not enough, and this is the whole
    /// subtlety of the gesture: the far side keeps its own idea of where the
    /// cursor is and repaints by absolute position, so a screen wiped behind
    /// its back gets the next prompt painted back down at the row it had
    /// reached, with the cleared rows blank above it. The only thing that
    /// resets that idea is a clear-screen from the far side itself, so the
    /// clear is asked of it - of IRIS at an idle prompt, of a shell at any
    /// moment at all - and the echo of the ask and the pre-clear screen are
    /// both dropped, so nothing of it is left on screen or in the scrollback.
    ///
    /// Anywhere else - mid-line or mid-routine on IRIS, or on a session that
    /// has ended - the command could be swallowed as input or appended to what
    /// is being typed, so the grid is cleared locally instead, and the prompt
    /// stays where IRIS left it.
    ///
    /// Nothing is said about any of this in the status line. The gesture is a
    /// frequent one, and a note that appears under every clear costs a row of
    /// the terminal to say what the screen has already shown; what it does is
    /// on the right-click entry's tooltip instead.
    pub(super) fn clear_active_terminal(&mut self) {
        self.clear_terminal(self.focused_at());
    }

    /// [`App::clear_active_terminal`] for one named pane, since with a tab
    /// split the right-click menu can be opened over either of them.
    pub(super) fn clear_terminal(&mut self, at: At) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        tab.view.clear_selection();
        tab.view.scroll_to_bottom();

        // Safe to ask the far side to clear itself: on IRIS that means an
        // idle prompt - it is reading a command line and nothing has been
        // typed on it yet - because the ask is a command line of its own. A
        // shell is sent a keystroke instead, which no `read` can swallow and
        // nothing half typed can absorb, so there is no moment at which it is
        // the wrong thing to send.
        let can_ask = tab.profile.is_shell()
            || lineedit::current(&tab.grid).is_some_and(|line| line.is_empty());
        if tab.session.is_some() && can_ask {
            tab.request_clear();
        } else {
            tab.grid.hard_reset();
        }
    }

    /// Remembers the command sitting on the active tab's prompt line.
    ///
    /// Read off the screen rather than accumulated from keystrokes, so a line
    /// that arrived by paste, or that IRIS recalled itself, counts exactly like
    /// a typed one. A row with no IRIS prompt on it is not a command line and
    /// is not recorded - which is also what keeps a password out of the file,
    /// since a credential prompt carries no `>` and is never echoed.
    /// `unechoed` is text typed in the same frame as the Enter, which IRIS has
    /// not sent back yet and which the screen therefore does not show.
    /// Returns whether the line qualified as a command and was recorded.
    pub(super) fn record_command(&mut self, at: At, unechoed: &str) -> bool {
        let Some(tab) = self.pane_mut(at) else {
            return false;
        };
        tab.recall_step = None;
        if tab.autologon.state() == AutoState::WaitPassword {
            return false;
        }
        let Some(text) = lineedit::typed_text(&tab.grid) else {
            return false;
        };
        let command = format!("{text}{unechoed}");
        // Twice over, because the two lists answer different questions: this
        // session's own recall, and what the next start will inherit.
        tab.remember_command(&command);
        self.history.record(&command);
        true
    }

    /// The easter egg: `/snake` submitted at a local IRIS prompt opens the
    /// game instead of reaching IRIS, which would only answer `<SYNTAX>`.
    ///
    /// Local sessions only. A remote server is somebody else's machine and a
    /// shell has a filesystem where `/snake` could mean something; the local
    /// instance is the one place the line is unambiguously a joke rather than
    /// a command. Nothing is recorded either - the line never ran, so it has
    /// no business in the recall or in `history.txt`.
    ///
    /// The line is rubbed out on the way, because the Enter that asked for it
    /// is never sent: IRIS is still reading the line it echoed, and leaving
    /// `/snake` sitting on the prompt would have the next thing typed run as
    /// part of it. Answers whether the line was the egg.
    pub(super) fn take_easter_egg(&mut self, at: At, unechoed: &str) -> bool {
        let Some(tab) = self.pane(at) else {
            return false;
        };
        if tab.profile.shell.is_some() || tab.profile.remote.is_some() {
            return false;
        }
        let Some(line) = lineedit::current(&tab.grid) else {
            return false;
        };
        let Some(typed) = lineedit::typed_text(&tab.grid) else {
            return false;
        };
        if !is_easter_egg(&format!("{typed}{unechoed}")) {
            return false;
        }

        // To the end of the line, then rub out every character of it - the
        // same gesture as a recall, which is the only way to change a buffer
        // the far side owns. The characters typed in this same frame are in
        // that buffer too but not yet on screen, so they are counted
        // separately: rubbing out only what the screen shows would leave the
        // last letter of `/snake` on the prompt.
        let app_cursor = tab.grid.app_cursor_keys;
        let rubouts = line.len() + unechoed.chars().count();
        let mut wire = cursor_bytes(line.end as i64 - line.cursor as i64, app_cursor);
        wire.extend(std::iter::repeat_n(0x7f, rubouts));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.view.clear_selection();
            tab.recall_step = None;
            tab.send(&wire);
        }
        self.open_snake_tab();
        true
    }

    /// Remembers the commands a multi-line paste is about to submit.
    ///
    /// A paste carrying line breaks presses Enter for the user, once per break,
    /// and none of those go through the key handling that records a typed
    /// command - so without this the lines run but recall never offers them
    /// again. The screen cannot be read for them either: the paste is recorded
    /// before IRIS has echoed any of it, so only the first line, which lands on
    /// the prompt already on screen, comes off the grid; the rest are the
    /// pasted text itself. Whatever follows the last break stays on the line
    /// unsubmitted and is recorded later, when the user presses Enter.
    pub(super) fn record_pasted(&mut self, at: At, text: &str) {
        let Some((first, rest)) = history::pasted_commands(text) else {
            return;
        };
        if !self.record_command(at, first) {
            // Not a command line - a password prompt, or no prompt at all.
            return;
        }
        for command in rest {
            if let Some(tab) = self.pane_mut(at) {
                tab.remember_command(command);
            }
            self.history.record(command);
        }
    }

    /// Replaces the line being typed with the next command from the history.
    ///
    /// IRIS keeps its own recall, but only for as long as the process lives and
    /// only for what was typed into it; this is the app's - this session's own
    /// commands first, then the ones every session that came before left
    /// behind, when the setting allows. What another tab has typed while this
    /// one was open belongs to that tab and is not offered here.
    ///
    /// Done by rubbing the line out and typing the replacement, because IRIS
    /// owns the read buffer and the only way to change it is to send the keys
    /// that would have changed it.
    ///
    /// The cursor need not be at the end of the line: rubout erases backwards,
    /// so it is walked to the end first and the whole replacement goes out as
    /// one write. Whether Up mid-line recalls at all is
    /// `settings.recall_mid_line`, decided back in [`input::translate`].
    pub(super) fn recall(&mut self, at: At, direction: input::Recall) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        // The state could have moved off the command line between the key
        // press and here.
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let app_cursor = tab.grid.app_cursor_keys;
        let encoding = tab.profile.wire_encoding();
        let walked_to = tab.recall_step;

        let list = self.history.recall_list(&tab.commands);
        let next = match direction {
            // Already at the oldest command, or nothing recalled to walk
            // forward from: leave the line exactly as it is rather than
            // clearing it.
            input::Recall::Back => match history::step_back(walked_to, list.len()) {
                Some(next) => next,
                None => return,
            },
            input::Recall::Forward => match history::step_forward(walked_to) {
                Some(next) => next,
                None => return,
            },
        };

        // No step left is the line the user started on, which is an empty one.
        let text = next
            .and_then(|step| list.get(step))
            .copied()
            .unwrap_or_default()
            .to_string();

        let mut wire = cursor_bytes(line.end as i64 - line.cursor as i64, app_cursor);
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        wire.extend_from_slice(&encoding.encode(&text));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.recall_step = next;
            tab.view.scroll_to_bottom();
        }
    }

    /// Selects the command being typed, so it can be copied or rubbed out in
    /// one gesture. Ctrl+A.
    pub(super) fn select_typed_line(&mut self, at: At) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid).filter(|l| !l.is_empty()) else {
            return;
        };
        let row = tab.grid.scrollback.len() + tab.grid.cursor.row;
        tab.view.selection = Some(Selection::across(row, line.start, line.end));
    }

    /// Drags the loose end of the selection over the command line, the way
    /// Shift plus a movement key does in a text field.
    ///
    /// The anchor is where the selection was started from - IRIS's cursor, the
    /// first time - and only the other end moves, so shift-left and then
    /// shift-right walks back over what was just selected. Nothing is sent:
    /// IRIS's cursor stays put and only the highlight moves.
    pub(super) fn extend_selection(&mut self, at: At, motion: Motion) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let row = tab.grid.scrollback.len() + tab.grid.cursor.row;

        // An existing selection on this line continues; anything else - none at
        // all, or one left over in the scrollback - starts again from the
        // cursor.
        let (anchor, focus) = tab
            .view
            .selection
            .filter(|s| s.span_on(row).is_some())
            .map(|s| (s.start.1, s.end.1))
            .unwrap_or((line.cursor, line.cursor));
        let anchor = anchor.clamp(line.start, line.end);
        let focus = lineedit::target(&tab.grid, line, focus, motion);

        // Walking the loose end back onto the anchor selects nothing, which is
        // no selection at all rather than an empty one - an empty selection
        // would go on quietly claiming Ctrl+C.
        tab.view.selection = (focus != anchor).then_some(Selection {
            start: (row, anchor),
            end: (row, focus),
        });
    }

    /// Walks IRIS's cursor over the command line by a whole motion - a word at
    /// a time, for Ctrl plus an arrow.
    ///
    /// Only the app knows where the word boundaries are, since only the app can
    /// see the line; IRIS is then told in the one language it acts on, which is
    /// arrow keys. The selection goes, exactly as it would in a text field when
    /// the cursor walks away from it.
    pub(super) fn move_by(&mut self, at: At, motion: Motion) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let to = lineedit::target(&tab.grid, line, line.cursor, motion);
        let columns = to as i64 - line.cursor as i64;
        if let Some(tab) = self.pane_mut(at) {
            tab.view.clear_selection();
        }
        self.move_cursor(at, columns);
    }

    /// Wraps the selected part of the command line in `open` and its closing
    /// character, leaving the same text selected.
    ///
    /// The whole line is rewritten rather than the two characters inserted in
    /// place. Inserting would mean trusting IRIS to be in insert rather than
    /// replace mode, which it never reports - see `Grid::insert_mode` - and
    /// getting that wrong would silently eat the two characters either side of
    /// the selection. Retyping the line is the same thing `App::recall` does
    /// and needs no such guess.
    ///
    /// Answers whether it happened, so the caller knows not to also type the
    /// character that asked for it.
    pub(super) fn surround_selection(&mut self, at: At, open: char) -> bool {
        let Some(tab) = self.pane(at) else {
            return false;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return false;
        };
        let Some((from, to)) = selection_in_line(tab, line) else {
            return false;
        };
        let Some(text) = lineedit::typed_text(&tab.grid) else {
            return false;
        };
        let Some((wrapped, span)) = input::surround(&text, line.start, from, to, open) else {
            return false;
        };
        let app_cursor = tab.grid.app_cursor_keys;
        let encoding = tab.profile.wire_encoding();
        let row = tab.grid.scrollback.len() + tab.grid.cursor.row;

        // To the end of the line, rub the whole of it out, then type it back
        // with the pair in place - one write, so the line is never half-way
        // between the two versions.
        let mut wire = cursor_bytes(line.end as i64 - line.cursor as i64, app_cursor);
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        wire.extend_from_slice(&encoding.encode(&wrapped));
        let wire = self.plugins.on_input(&wire);

        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            // The same characters, one column further right, so typing a second
            // quote or bracket wraps what is already wrapped.
            tab.view.selection = Some(crate::ui::terminal_view::Selection {
                start: (row, span.0),
                end: (row, span.1),
            });
            tab.recall_step = None;
            tab.view.scroll_to_bottom();
        }
        true
    }

    /// Rubs the selected part of the command line out of IRIS's read buffer.
    ///
    /// Rubout erases the character *before* the cursor, so the cursor is walked
    /// to the end of the selection first and the whole thing goes out as one
    /// write - a half-applied erase would leave the line in a state neither
    /// side agrees on.
    pub(super) fn erase_selection(&mut self, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let Some((from, to)) = selection_in_line(tab, line) else {
            return;
        };

        let mut wire = cursor_bytes(to as i64 - line.cursor as i64, tab.grid.app_cursor_keys);
        wire.extend(std::iter::repeat_n(0x7f, to - from));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.view.clear_selection();
            tab.recall_step = None;
        }
    }

    /// Walks IRIS's cursor `columns` to the right, or to the left when
    /// negative, with the arrow keys it does act on.
    pub(super) fn move_cursor(&mut self, at: At, columns: i64) {
        let app_cursor = self.pane(at).is_some_and(|tab| tab.grid.app_cursor_keys);
        let wire = cursor_bytes(columns, app_cursor);
        if wire.is_empty() {
            return;
        }
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane(at) {
            tab.send(&wire);
        }
    }

    /// Puts one pane's selection on the clipboard, if it has one.
    pub(super) fn copy_selection(&self, ctx: &Context, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        if let Some(text) = tab.view.selected_text(&tab.grid) {
            ctx.copy_text(text);
        }
    }

    /// Copies the selection and types it straight back into the same session.
    ///
    /// The gesture this terminal is actually asked for: a global name, a
    /// routine label or an error location is on screen, and it has to reach the
    /// command line. By hand that is a copy, a click into the prompt and a
    /// paste. The clipboard is filled all the same, so what was picked up is
    /// still available to whatever else the user is working in.
    ///
    /// It goes through the same sanitiser a real paste does, so a selection
    /// spanning two lines arrives as two lines - which at a prompt means IRIS
    /// runs the first of them. That is what pasting the same text does, and
    /// pretending otherwise would be a different gesture with the same name.
    pub(super) fn copy_and_paste(&mut self, ctx: &Context, at: At) {
        let Some((text, encoding)) = self.pane(at).and_then(|tab| {
            Some((
                tab.view.selected_text(&tab.grid)?,
                tab.profile.wire_encoding(),
            ))
        }) else {
            return;
        };
        ctx.copy_text(text.clone());
        let pasted = input::sanitize_paste(&text);
        let wire = self.plugins.on_input(&encoding.encode(&pasted));
        self.record_pasted(at, &pasted);
        if let Some(tab) = self.pane_mut(at) {
            // The same two things typing does: what the recall had walked to is
            // no longer where the line came from, and the view returns to the
            // live output.
            tab.recall_step = None;
            tab.view.scroll_to_bottom();
            tab.send(&wire);
        }
    }

    pub(super) fn send_lines_to_active(&mut self, lines: &[String]) {
        let at = self.focused_at();
        let Some(tab) = self.pane(at) else {
            self.set_status(tr("No active session."));
            return;
        };
        if tab.session.is_none() {
            self.set_status(tr("That session has ended."));
            return;
        }
        // Whatever was half-typed goes first. A macro is a whole command, and
        // appended to the tail of an unfinished one it is not the command
        // either of them was: IRIS would read `write 1ZWRITE ^CSW1`.
        self.clear_typed_line(at);
        if let Some(tab) = self.pane(at) {
            tab.send_lines(lines);
        }
    }

    /// Rubs out whatever is sitting on a pane's prompt line, so something else
    /// can be sent to it.
    ///
    /// The same mechanics as [`App::recall`]: the cursor is walked to the end
    /// of the line first, because rubout erases backwards, and the whole thing
    /// goes out as one write. Does nothing off a command line - a routine
    /// reading a line of its own owns those keys.
    pub(super) fn clear_typed_line(&mut self, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid).filter(|line| !line.is_empty()) else {
            return;
        };
        let mut wire = cursor_bytes(
            line.end as i64 - line.cursor as i64,
            tab.grid.app_cursor_keys,
        );
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.recall_step = None;
        }
    }

    pub(super) fn export(&mut self, range: Range, format: export::Format) {
        let Some(tab) = self.active_tab() else {
            self.set_status(tr("No active session to export."));
            return;
        };
        let theme = self.theme();
        let contents = match format {
            export::Format::Text => export::to_text(&tab.grid, range),
            export::Format::Html => export::to_html(&tab.grid, range, &theme),
        };
        let name = export::suggested_name(&tab.profile.name, format);
        let path = self.settings.log_dir.join(name);

        match export::write_file(&path, &contents) {
            Ok(()) => self.set_status(tr1("Exported to {}", &path.display().to_string())),
            Err(e) => self.set_status(tr1("Export failed: {}", &format!("{e:#}"))),
        }
    }

    /// Hands this session over to Claude Code, with `panes` deciding whether
    /// the other half of a split goes with it.
    ///
    /// The focused pane is always in the file. A split is usually two halves of
    /// one problem - a routine running on one side, a global inspected on the
    /// other - so `Both` is there for when leaving one out is what would make
    /// the answer useless. On a tab that is not split it means the same thing
    /// as `Focused`.
    pub(super) fn analyze_with_claude(
        &mut self,
        at: At,
        scope: analyze::Scope,
        panes: analyze::Panes,
    ) {
        if self.pane(at).is_none() {
            self.set_status(tr("No active session."));
            return;
        }
        // Both panes in the order they are on screen, so "pane 1" in the file
        // is the pane called `1:` in the tab strip - rather than in the order
        // the keyboard happens to be in.
        let wanted: Vec<At> = match panes {
            analyze::Panes::Both if self.tabs.get(at.tab).is_some_and(|t| t.split.is_some()) => {
                vec![
                    At {
                        tab: at.tab,
                        pane: Pane::First,
                    },
                    At {
                        tab: at.tab,
                        pane: Pane::Second,
                    },
                ]
            }
            _ => vec![at],
        };

        // The endpoint and the selection come out in one pass, and the sources
        // borrow them: both are owned values a `Source` only points at, so they
        // have to outlive it.
        let captured: Vec<(&Tab, String, Option<String>)> = wanted
            .iter()
            .filter_map(|&at| self.pane(at))
            .map(|tab| {
                (
                    tab,
                    tab.profile.endpoint(),
                    tab.view.selected_text(&tab.grid),
                )
            })
            .collect();
        let sources: Vec<analyze::Source> = captured
            .iter()
            .map(|(tab, endpoint, selection)| analyze::Source {
                endpoint,
                grid: &tab.grid,
                selection: selection.as_deref(),
            })
            .collect();

        let path = match analyze::write_context(&sources, scope) {
            Ok(path) => path,
            Err(e) => {
                self.set_status(tr1("Could not prepare the output: {}", &format!("{e:#}")));
                return;
            }
        };
        // A tab of its own rather than a window of the operating system's: the
        // conversation is about what is on the terminal, so it belongs beside
        // it. A `claude` that is not installed fails the way any other shell
        // tab does - the tab opens and says what went wrong - which is more
        // use than a status line, because the message is in front of the file
        // it was going to read.
        self.open_tab(analyze::tab_profile(&path));
        self.set_status(tr1(
            "Claude is opening with this output in context ({}). Ask it whatever you like.",
            &path.display().to_string(),
        ));
    }
}

/// What opens the easter egg.
///
/// Spelled like a slash command because that is what it is pretending to be,
/// and matched with the surrounding blanks trimmed off: a prompt line is read
/// back off the screen, and a trailing space is indistinguishable from the
/// blank the cursor is sitting on.
const SNAKE_COMMAND: &str = "/snake";

fn is_easter_egg(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case(SNAKE_COMMAND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_egg_is_the_whole_line_and_nothing_else() {
        assert!(is_easter_egg("/snake"));
        assert!(is_easter_egg("  /snake "), "read back off a padded row");
        assert!(is_easter_egg("/SNAKE"));
        assert!(!is_easter_egg("w /snake"));
        assert!(!is_easter_egg("/snakes"));
        assert!(!is_easter_egg("snake"));
        assert!(!is_easter_egg(""));
    }
}
