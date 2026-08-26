//! Exporting terminal output to a file or the clipboard.
//!
//! Shares the grid's row iterator with the clean logger, so what you export is
//! exactly what a transcript would have recorded.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Theme;
use crate::term::palette;
use crate::term::{Grid, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// Just what is on screen now.
    Screen,
    /// Scrollback plus screen.
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Plain text, trailing blanks trimmed.
    Text,
    /// HTML preserving foreground/background colours.
    Html,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Html => "html",
        }
    }
}

/// Rows covered by `range`, oldest first.
fn rows(grid: &Grid, range: Range) -> Vec<&Row> {
    match range {
        Range::Screen => grid.screen.iter().collect(),
        Range::All => grid.scrollback.iter().chain(grid.screen.iter()).collect(),
    }
}

pub fn to_text(grid: &Grid, range: Range) -> String {
    let mut out = rows(grid, range)
        .into_iter()
        .map(Row::to_text)
        .collect::<Vec<_>>()
        .join("\n");
    // A terminal screen is mostly blank at the bottom; exporting a page of
    // empty lines is noise.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

/// Renders to standalone HTML with the theme's colours baked in, so the file
/// looks like the terminal did when it was exported.
pub fn to_html(grid: &Grid, range: Range, theme: &Theme) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\">\n");
    out.push_str("<title>IRIS session</title>\n<style>\n");
    out.push_str(&format!(
        "body {{ background: {}; color: {}; }}\n",
        css(theme.background),
        css(theme.foreground)
    ));
    out.push_str(
        "pre { font-family: Consolas, \"DejaVu Sans Mono\", Menlo, monospace; \
                  font-size: 13px; line-height: 1.25; margin: 1rem; }\n",
    );
    out.push_str("</style></head><body><pre>");

    for row in rows(grid, range) {
        let end = row
            .cells
            .iter()
            .rposition(|c| !c.is_blank())
            .map_or(0, |i| i + 1);

        let mut col = 0;
        while col < end {
            let (fg, bg) = palette::resolve(&row.cells[col], theme);
            // Batch runs of identical styling into one span, exactly as the
            // renderer batches draw calls.
            let mut run_end = col + 1;
            while run_end < end {
                let (next_fg, next_bg) = palette::resolve(&row.cells[run_end], theme);
                if next_fg != fg || next_bg != bg {
                    break;
                }
                run_end += 1;
            }

            let text: String = row.cells[col..run_end].iter().map(|c| c.ch).collect();
            let escaped = escape_html(&text);

            if fg == theme.foreground && bg == theme.background {
                out.push_str(&escaped);
            } else {
                out.push_str(&format!(
                    "<span style=\"color:{};background:{}\">{escaped}</span>",
                    css(fg),
                    css(bg)
                ));
            }
            col = run_end;
        }
        out.push('\n');
    }

    out.push_str("</pre></body></html>\n");
    out
}

pub fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// A default filename for an export, stamped so repeated exports do not
/// silently overwrite one another.
pub fn suggested_name(profile: &str, format: Format) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let safe: String = profile
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{safe}-{stamp}.{}", format.extension())
}

fn css(color: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::parser;

    fn session(input: &[u8], cols: usize, rows: usize) -> Grid {
        let mut grid = Grid::new(cols, rows, 100);
        let mut vte = vte::Parser::new();
        parser::advance(&mut vte, &mut grid, input);
        grid
    }

    #[test]
    fn screen_export_covers_only_the_visible_screen() {
        let grid = session(b"one\r\ntwo\r\nthree", 20, 2);
        assert_eq!(to_text(&grid, Range::Screen), "two\nthree\n");
    }

    #[test]
    fn all_export_includes_scrollback() {
        let grid = session(b"one\r\ntwo\r\nthree", 20, 2);
        assert_eq!(to_text(&grid, Range::All), "one\ntwo\nthree\n");
    }

    /// Exporting must match what the clean logger would have recorded.
    #[test]
    fn text_export_agrees_with_the_grids_own_line_rendering() {
        let grid = session(b"alpha\r\nbeta", 20, 3);
        let expected = grid.all_text().join("\n");
        assert!(to_text(&grid, Range::All).starts_with(expected.trim_end()));
    }

    #[test]
    fn trailing_blank_lines_are_trimmed_to_a_single_newline() {
        let grid = session(b"only", 20, 10);
        assert_eq!(to_text(&grid, Range::Screen), "only\n");
    }

    #[test]
    fn html_escapes_markup_characters() {
        let grid = session(b"a<b>&c", 20, 1);
        let html = to_html(&grid, Range::Screen, &Theme::default());
        assert!(html.contains("a&lt;b&gt;&amp;c"), "got: {html}");
        assert!(!html.contains("<b>"));
    }

    #[test]
    fn html_emits_a_span_for_coloured_text_only() {
        let grid = session(b"plain\r\n\x1b[31mred\x1b[0m", 20, 2);
        let html = to_html(&grid, Range::Screen, &Theme::default());
        assert!(html.contains("<span style=\"color:"), "no span in {html}");
        // The unstyled line should not be wrapped.
        assert!(html.contains("plain\n") || html.contains("plain<"));
    }

    #[test]
    fn html_is_a_complete_standalone_document() {
        let grid = session(b"x", 10, 1);
        let html = to_html(&grid, Range::Screen, &Theme::default());
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.trim_end().ends_with("</html>"));
    }

    #[test]
    fn suggested_names_carry_the_right_extension_and_are_path_safe() {
        assert!(suggested_name("RDB/1", Format::Text).ends_with(".txt"));
        assert!(suggested_name("RDB/1", Format::Html).ends_with(".html"));
        assert!(!suggested_name("RDB/1", Format::Text).contains('/'));
    }
}
