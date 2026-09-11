//! Finding text in what a session has already printed.
//!
//! The transcript is the reason this terminal keeps a scrollback at all - a
//! `zwrite` from an hour ago is evidence - and until now the only way to look
//! through it was to export it and open the file in something else. Ctrl+F
//! searches it in place.
//!
//! The search runs over the grid, not over the raw bytes: what is looked
//! through is exactly what is on screen, so a match's position is a line and a
//! column that the renderer can highlight and the view can be scrolled to. That
//! also means a line IRIS cut at the right margin is searched as the cut line,
//! which is the honest answer - the rest of it never arrived.
//!
//! Deliberately free of egui, apart from [`bar`]: [`find`] is the part worth
//! testing, and it is testable only if it can be called without a window.

use egui::{Key, Ui};

use crate::config::theme::Theme;
use crate::i18n::{tr, tr2};
use crate::term::Grid;

/// One hit: the line it is on and the columns it covers, end exclusive.
///
/// Columns, not byte offsets. A grid cell holds one character however many
/// bytes it takes, and the ERP's text is full of accented ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub from: usize,
    pub to: usize,
}

impl Match {
    /// Whether `col` on `line` is inside this hit.
    pub fn covers(&self, line: usize, col: usize) -> bool {
        self.line == line && col >= self.from && col < self.to
    }
}

/// What the find bar asked the caller to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Bring the current hit on screen.
    Reveal,
    /// Close the bar and give the keyboard back to the terminal.
    Close,
}

/// Per-tab find state, kept between frames.
///
/// Lives beside the tab's scroll position rather than on the app, because a
/// search belongs to the transcript it was made in: switching tabs and back
/// finds the same text still highlighted.
#[derive(Default)]
pub struct Search {
    pub open: bool,
    pub query: String,
    pub case_sensitive: bool,
    /// Every hit, in reading order.
    pub matches: Vec<Match>,
    /// Which of them is the one being looked at, if there are any.
    pub current: usize,
    /// The text field should take the keyboard on the next frame - set when the
    /// bar is opened, and when Ctrl+F is pressed with it already open.
    pub focus_field: bool,
    /// What `matches` was computed from: the grid's revision and the query.
    /// Re-running the scan when none of them has changed would mean searching
    /// the whole scrollback every frame the window repainted.
    seen: Option<(u64, String, bool)>,
}

impl Search {
    /// Opens the bar, or re-focuses it if it is already open. Ctrl+F in an
    /// editor does both, and the second is what reaches for a new search
    /// without having to close the bar first.
    pub fn open(&mut self) {
        self.open = true;
        self.focus_field = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus_field = false;
        self.matches.clear();
        self.seen = None;
    }

    /// The hit being looked at, if there is one.
    pub fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// Re-runs the search if anything it depends on has changed.
    ///
    /// The current hit is kept pointing at the same place in the transcript
    /// where it can be: output arriving while the bar is open renumbers nothing
    /// above it, but a line rotating out of the scrollback shifts everything
    /// down by one, so it is matched by position rather than by index.
    pub fn refresh(&mut self, grid: &Grid) {
        let key = (grid.revision, self.query.clone(), self.case_sensitive);
        if self.seen.as_ref() == Some(&key) {
            return;
        }
        let was = self.current_match();
        self.matches = find(grid, &self.query, self.case_sensitive);
        self.current = was
            .and_then(|was| {
                self.matches
                    .iter()
                    .position(|m| m.line == was.line && m.from == was.from)
            })
            .unwrap_or(0)
            .min(self.matches.len().saturating_sub(1));
        self.seen = Some(key);
    }

    /// Steps to the next hit, or the previous one, wrapping round at each end.
    ///
    /// Wrapping rather than stopping: a search over a transcript has no natural
    /// end to stop at, and every editor wraps.
    pub fn step(&mut self, forward: bool) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() - 1;
        self.current = if forward {
            if self.current >= last {
                0
            } else {
                self.current + 1
            }
        } else if self.current == 0 {
            last
        } else {
            self.current - 1
        };
    }

    /// Hits on one line, for the renderer. Empty for the overwhelming majority
    /// of lines, so the cost of asking is a binary search and nothing else.
    pub fn on_line(&self, line: usize) -> &[Match] {
        let start = self.matches.partition_point(|m| m.line < line);
        let end = self.matches.partition_point(|m| m.line <= line);
        &self.matches[start..end]
    }
}

/// Every occurrence of `query` in the grid, scrollback first, in reading order.
///
/// Case-insensitive unless asked otherwise, which is the right default for a
/// terminal: IRIS answers in upper case far more often than anyone types it.
///
/// An empty query matches nothing rather than everything - the bar has just
/// been opened and nothing has been typed into it yet.
pub fn find(grid: &Grid, query: &str, case_sensitive: bool) -> Vec<Match> {
    let needle: Vec<char> = fold(query, case_sensitive);
    if needle.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for line in 0..grid.total_lines() {
        let Some(row) = grid.line(line) else {
            continue;
        };
        // One character per column, so an index into this is a column - which
        // is what the renderer and the scroll both need back.
        let hay: Vec<char> = row.cells[..row.used_width()]
            .iter()
            .map(|c| fold_char(c.ch, case_sensitive))
            .collect();
        if hay.len() < needle.len() {
            continue;
        }
        let mut at = 0;
        while at + needle.len() <= hay.len() {
            if hay[at..at + needle.len()] == needle[..] {
                out.push(Match {
                    line,
                    from: at,
                    to: at + needle.len(),
                });
                // Hits do not overlap: the next one starts after this one, the
                // way every find does.
                at += needle.len();
            } else {
                at += 1;
            }
        }
    }
    out
}

fn fold(text: &str, case_sensitive: bool) -> Vec<char> {
    text.chars().map(|c| fold_char(c, case_sensitive)).collect()
}

/// One character, case-folded unless the search is case-sensitive.
///
/// `to_lowercase` can answer with more than one character - the German sharp s
/// is the classic - and a grid column holds exactly one, so the first is taken.
/// That makes such a character fold to itself on both sides of the comparison,
/// which is all the search needs.
fn fold_char(ch: char, case_sensitive: bool) -> char {
    if case_sensitive {
        ch
    } else {
        ch.to_lowercase().next().unwrap_or(ch)
    }
}

/// Draws the find bar and handles the keys that belong to it.
///
/// Returns what the caller has to act on: the caller owns the view, so it is
/// the one that can scroll a hit into sight.
pub fn bar(ui: &mut Ui, search: &mut Search, theme: &Theme) -> Option<Action> {
    let mut action = None;
    let mut step_forward = None;

    ui.horizontal(|ui| {
        ui.label(tr("Find"));

        let field = egui::TextEdit::singleline(&mut search.query)
            .desired_width(220.0)
            .hint_text(tr("text in the output"));
        let response = ui.add(field);

        if std::mem::take(&mut search.focus_field) {
            response.request_focus();
            // Selected, so that Ctrl+F with the bar already open replaces the
            // last search by typing rather than appending to it.
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(search.query.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), response.id);
            }
        }

        // Enter walks the hits without leaving the field, which is what the
        // hands expect; Shift+Enter walks back.
        if response.has_focus() {
            let (enter, shift, escape) = ui.input(|i| {
                (
                    i.key_pressed(Key::Enter),
                    i.modifiers.shift,
                    i.key_pressed(Key::Escape),
                )
            });
            if enter {
                step_forward = Some(!shift);
            }
            if escape {
                action = Some(Action::Close);
            }
        }

        if ui
            .small_button("<")
            .on_hover_text(tr("Previous match (Shift+Enter)"))
            .clicked()
        {
            step_forward = Some(false);
        }
        if ui
            .small_button(">")
            .on_hover_text(tr("Next match (Enter)"))
            .clicked()
        {
            step_forward = Some(true);
        }

        if ui
            .selectable_label(search.case_sensitive, tr("Aa"))
            .on_hover_text(tr("Match upper and lower case exactly"))
            .clicked()
        {
            search.case_sensitive = !search.case_sensitive;
        }

        // The count, which is also the only report that a search found nothing:
        // an empty query says nothing at all, because nothing has been asked
        // yet.
        let label = if search.query.is_empty() {
            String::new()
        } else if search.matches.is_empty() {
            tr("no matches").to_string()
        } else {
            tr2(
                "{} of {}",
                &(search.current + 1).to_string(),
                &search.matches.len().to_string(),
            )
        };
        if !label.is_empty() {
            ui.colored_label(theme.foreground, label);
        }

        if ui.small_button(tr("Close")).clicked() {
            action = Some(Action::Close);
        }
    });

    if let Some(forward) = step_forward {
        search.step(forward);
        action = Some(Action::Reveal);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid holding one line of text per entry, as the scrollback would.
    fn grid_of(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(200, lines.len().max(1), 100);
        for (row, text) in lines.iter().enumerate() {
            grid.screen[row].set_text(text);
        }
        grid
    }

    #[test]
    fn a_match_is_reported_as_the_columns_it_covers() {
        let grid = grid_of(&["set x=^GLOBAL"]);
        assert_eq!(
            find(&grid, "^GLOBAL", true),
            vec![Match {
                line: 0,
                from: 6,
                to: 13
            }]
        );
    }

    /// The default, and the one that matters here: IRIS answers in upper case
    /// far more often than anyone types it.
    #[test]
    fn the_search_ignores_case_unless_told_not_to() {
        let grid = grid_of(&["<UNDEFINED> zTest+4^Rotina"]);
        assert_eq!(find(&grid, "undefined", false).len(), 1);
        assert!(find(&grid, "undefined", true).is_empty());
        assert_eq!(find(&grid, "UNDEFINED", true).len(), 1);
    }

    #[test]
    fn every_occurrence_is_found_in_reading_order() {
        let grid = grid_of(&["a b a", "c", "a"]);
        assert_eq!(
            find(&grid, "a", false),
            vec![
                Match {
                    line: 0,
                    from: 0,
                    to: 1
                },
                Match {
                    line: 0,
                    from: 4,
                    to: 5
                },
                Match {
                    line: 2,
                    from: 0,
                    to: 1
                },
            ]
        );
    }

    /// Overlapping hits are not reported twice, the same rule every find has.
    #[test]
    fn hits_do_not_overlap() {
        let grid = grid_of(&["aaaa"]);
        let hits = find(&grid, "aa", true);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].from, 0);
        assert_eq!(hits[1].from, 2);
    }

    /// Columns are characters, not bytes, or a highlight would land on the
    /// wrong cell the moment the line had an accent earlier on it.
    #[test]
    fn columns_are_counted_in_characters() {
        let grid = grid_of(&["ação total"]);
        assert_eq!(
            find(&grid, "total", false),
            vec![Match {
                line: 0,
                from: 5,
                to: 10
            }]
        );
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        let grid = grid_of(&["anything at all"]);
        assert!(find(&grid, "", false).is_empty());
        assert!(find(&grid, "", true).is_empty());
    }

    /// A query longer than the line it is being looked for in must not panic.
    #[test]
    fn a_query_longer_than_the_text_finds_nothing() {
        let grid = grid_of(&["ab"]);
        assert!(find(&grid, "abcdef", false).is_empty());
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut search = Search {
            matches: (0..3)
                .map(|line| Match {
                    line,
                    from: 0,
                    to: 1,
                })
                .collect(),
            ..Search::default()
        };

        search.step(true);
        assert_eq!(search.current, 1);
        search.step(true);
        assert_eq!(search.current, 2);
        search.step(true);
        assert_eq!(search.current, 0, "past the last hit is the first");
        search.step(false);
        assert_eq!(search.current, 2, "and back the other way");
    }

    /// Nothing found: stepping is a no-op rather than an index off the end.
    #[test]
    fn stepping_with_nothing_found_stays_put() {
        let mut search = Search::default();
        search.step(true);
        search.step(false);
        assert_eq!(search.current, 0);
        assert_eq!(search.current_match(), None);
    }

    #[test]
    fn the_hits_on_one_line_are_the_ones_the_renderer_is_given() {
        let grid = grid_of(&["a", "a a", "b"]);
        let mut search = Search {
            query: "a".into(),
            ..Search::default()
        };
        search.refresh(&grid);

        assert_eq!(search.on_line(0).len(), 1);
        assert_eq!(search.on_line(1).len(), 2);
        assert!(search.on_line(2).is_empty());
        assert!(search.on_line(99).is_empty());
    }

    /// The scan is skipped when nothing it depends on has changed - otherwise
    /// every repaint of an idle window would search the whole scrollback.
    #[test]
    fn a_refresh_that_changes_nothing_does_not_rescan() {
        let mut grid = grid_of(&["hello"]);
        let mut search = Search {
            query: "hello".into(),
            ..Search::default()
        };
        search.refresh(&grid);
        assert_eq!(search.matches.len(), 1);

        // Forced out of step behind its back: a second refresh must not notice,
        // because the revision and the query are both unchanged.
        search.matches.clear();
        search.refresh(&grid);
        assert!(search.matches.is_empty(), "should not have rescanned");

        // A touched grid is a changed one.
        grid.touch();
        search.refresh(&grid);
        assert_eq!(search.matches.len(), 1, "a new revision rescans");
    }

    /// Output arriving while the bar is open must not move the hit being looked
    /// at onto a different one.
    #[test]
    fn the_current_hit_survives_a_rescan() {
        let grid = grid_of(&["a", "a", "a"]);
        let mut search = Search {
            query: "a".into(),
            ..Search::default()
        };
        search.refresh(&grid);
        search.step(true);
        search.step(true);
        assert_eq!(search.current_match().map(|m| m.line), Some(2));

        let mut grown = grid;
        grown.touch();
        search.refresh(&grown);
        assert_eq!(
            search.current_match().map(|m| m.line),
            Some(2),
            "still looking at the same hit"
        );
    }
}
