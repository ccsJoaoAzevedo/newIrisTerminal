//! The Themes pages: every theme as a thumbnail, and an editor for every
//! colour one carries.
//!
//! Themes were TOML files and nothing else, so changing a colour meant finding
//! the folder, editing hex by hand, and restarting. This is the same file
//! format with a front end on it - and the same distinction the loader makes:
//! the built-ins live in the binary and are immutable, so the first thing
//! anyone does here is duplicate one.
//!
//! The gallery is the category's page, with the theme it is on previewed at
//! the top. The editor is a sub-page, and each group of colours a sub-page of
//! that - one thing on screen per page, the grid, the frame round it, the
//! title bar's controls, the highlighting - with the same live preview fixed
//! above them, so a dragged swatch is judged against real-looking output in
//! the frame it is dragged.
//!
//! Edits land in the themes as they are made, so the terminal behind the
//! window repaints in the colour being dragged; only what has to reach the
//! disk or the settings goes out, as a [`ThemeAction`], so a theme cannot be
//! saved or deleted from inside a paint pass.

use egui::color_picker::{color_edit_button_srgba, Alpha};
use egui::{pos2, vec2, Color32, Grid, Rect, Rounding, Sense, Stroke, Ui};

use super::{page, section, subpage, untitled, Category, Ctx, Item, Page, Route, Section};
use crate::config::theme::{
    GradientDirection, Theme, TitleButton, UiGradient, WindowButtonSlot as Slot, WindowButtonStyle,
};
use crate::i18n::{tr, tr1};
use crate::ui::panels::{UiRequest, WARNING};
use crate::ui::prefs::{self, Card, Row, RowResponse};

/// Something the Themes pages asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeAction {
    /// Make this theme the active one.
    Activate(String),
    /// Write this theme to its file, creating the file if it has none.
    Save(String),
    /// Forget this theme and delete its file.
    Delete(String),
    /// A colour of a theme was changed in place: draw the window in it again.
    /// Nothing is written - the save waits for the pointer to let go.
    Edited,
}

/// What the Themes pages keep between frames.
#[derive(Default)]
pub struct ThemePageState {
    /// Which theme the pages are on. `None` follows whatever is active, so
    /// opening Themes lands on the theme currently on screen.
    pub selected: Option<String>,
    /// The name being typed into the rename field, and which theme it belongs
    /// to. Kept out of the theme itself so a half-typed name is never a name.
    rename: Option<(String, String)>,
    /// A theme with unsaved edits.
    ///
    /// Dragging inside a colour picker reports a change every frame, and each
    /// one would otherwise be a file write; the save waits until the pointer is
    /// released, which is one write per adjustment.
    dirty: Option<String>,
    /// A user theme the delete button has been pressed on, waiting on the
    /// confirmation that a theme's colours are not recoverable.
    confirm_delete: Option<String>,
}

impl ThemePageState {
    /// The theme the pages should show, given what is active.
    fn showing(&self, themes: &[Theme], active: &str) -> Option<String> {
        let named = |name: &str| themes.iter().any(|t| t.name == name);
        self.selected
            .as_deref()
            .filter(|name| named(name))
            .or(Some(active).filter(|name| named(name)))
            .or_else(|| themes.first().map(|t| t.name.as_str()))
            .map(str::to_string)
    }
}

/// The index of the theme the pages are on.
fn editing(c: &Ctx<'_>) -> Option<usize> {
    let name = c.state.themes.showing(c.themes, &c.settings.theme)?;
    c.themes.iter().position(|t| t.name == name)
}

/// Whether the theme being edited passes `test` - for a row that is only
/// there when, say, a gradient is.
fn editing_has(c: &Ctx<'_>, test: impl Fn(&Theme) -> bool) -> bool {
    editing(c).is_some_and(|i| test(&c.themes[i]))
}

const EDITOR: Route = Route::sub(Category::Themes, "theme");

pub(super) fn pages() -> Vec<Page> {
    let group = |sub, title, sections| {
        subpage(Category::Themes, sub, title, sections)
            .under("theme")
            .with_top(preview_top)
    };
    vec![
        page(Category::Themes, gallery()).with_top(preview_top),
        subpage(Category::Themes, "theme", "Theme", editor())
            .with_heading(theme_title)
            .with_top(preview_top),
        group("theme_terminal", "Terminal colours", terminal_colours()),
        group("theme_scrollbar", "Scrollbar", scrollbar_colours()),
        group("theme_ansi", "ANSI", ansi_colours()),
        group("theme_chrome", "Chrome", chrome_colours()),
        group("theme_buttons", "Window buttons", button_colours()),
        group(
            "theme_order",
            "Title bar buttons, left to right",
            title_bar_order(),
        ),
        group("theme_syntax", "ObjectScript syntax", syntax_colours()),
    ]
}

fn theme_title(c: &Ctx<'_>) -> String {
    match editing(c) {
        Some(i) => c.themes[i].name.clone(),
        None => tr("Theme").to_owned(),
    }
}

/// After the page is drawn: what this frame's edits come to.
pub(super) fn settle(ctx: &egui::Context, c: &mut Ctx<'_>, open: bool) {
    if c.theme_edited {
        if let Some(name) = c.state.themes.showing(c.themes, &c.settings.theme) {
            c.state.themes.dirty = Some(name);
        }
        c.requests.push(UiRequest::Theme(ThemeAction::Edited));
    }
    // One write per adjustment rather than one per frame of a drag - and on
    // the way out, since closing the window is not a dialog's Cancel and
    // nothing else would ever write the edit.
    if !open || !ctx.input(|i| i.pointer.any_down()) {
        if let Some(name) = c.state.themes.dirty.take() {
            c.requests.push(UiRequest::Theme(ThemeAction::Save(name)));
        }
    }
}

// ---------------------------------------------------------------------------
// The gallery
// ---------------------------------------------------------------------------

fn gallery() -> Vec<Section> {
    vec![
        section(
            "Built-in",
            vec![Item::rows("themes_builtin", "Built-in themes", builtin_tiles)
                .keys(&["gallery", "thumbnail", "galeria", "miniatura", "dark", "light", "escuro", "claro"])],
        ),
        section(
            "My themes",
            vec![Item::rows("themes_mine", "My themes", my_tiles)
                .keys(&["custom", "own", "personalizado", "meus"])],
        )
        .footer("Click a theme to use it, double-click to edit its colours. The built-in themes are read-only: duplicate one to make it yours."),
        untitled(vec![
            Item::link("theme_edit_link", "Colours and title bar", EDITOR)
                .sub("Every colour of the selected theme, the gradients behind them, and the order of its title bar.")
                .keys(&["edit", "editar", "colour", "color", "cores", "objectscript", "syntax", "sintaxe", "gradient", "degrade"]),
            Item::rows("theme_actions", "Duplicate", theme_actions)
                .contextual()
                .keys(&["new", "delete", "copy", "novo", "nova", "excluir", "apagar", "copiar"]),
        ]),
        untitled(vec![Item::control("themes_folder", "Themes folder", themes_folder)
            .sub("The built-in themes are read-only; duplicating one gives you a copy to edit. A theme file dropped into the themes folder by hand is picked up at the next start.")
            .keys(&["folder", "pasta", "file", "arquivo"])]),
    ]
}

fn builtin_tiles(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    tiles(card, c, true);
}

fn my_tiles(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    tiles(card, c, false);
}

/// The width and height of a thumbnail, without its name.
const TILE: egui::Vec2 = vec2(132.0, 82.0);

/// The built-in themes, or the user's, as thumbnails. Clicking one uses it - a
/// colour scheme is judged against real output, not against a swatch - and
/// double-clicking one opens its editor.
fn tiles(card: &mut Card<'_>, c: &mut Ctx<'_>, builtin: bool) {
    let shown: Vec<usize> = (0..c.themes.len())
        .filter(|&i| c.themes[i].builtin == builtin)
        .collect();
    if shown.is_empty() {
        card.row(Row::new(tr("None yet - duplicate one.")), |_| ());
        return;
    }
    let showing = c.state.themes.showing(c.themes, &c.settings.theme);
    let active = c.settings.theme.clone();
    let themes = &*c.themes;
    let mut clicked = None;
    let mut opened = None;
    card.custom(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(14.0, 12.0);
            for &i in &shown {
                let theme = &themes[i];
                let name = theme.name.as_str();
                let response = tile(ui, theme, showing.as_deref() == Some(name), active == name);
                if response.clicked() {
                    clicked = Some(name.to_owned());
                }
                if response.double_clicked() {
                    opened = Some(name.to_owned());
                }
            }
        });
    });
    if let Some(name) = clicked.or(opened.clone()) {
        let state = &mut c.state.themes;
        state.confirm_delete = None;
        state.rename = None;
        state.selected = Some(name.clone());
        if name != active {
            c.requests
                .push(UiRequest::Theme(ThemeAction::Activate(name)));
        }
    }
    if opened.is_some() {
        c.go = Some(EDITOR);
    }
}

/// One thumbnail and the name under it, ringed in the accent when it is the
/// one the pages are on and marked when it is the one in use.
fn tile(ui: &mut Ui, theme: &Theme, selected: bool, active: bool) -> egui::Response {
    let palette = prefs::Palette::for_ui(ui);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_height = font.size + 6.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(TILE.x, TILE.y + label_height), Sense::click());
    let response = response.on_hover_text(if active {
        tr("In use").to_owned()
    } else {
        tr1("Use {}", &theme.name)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let thumb = Rect::from_min_size(rect.min, TILE);
    paint_thumbnail(ui.painter(), thumb, theme, palette.card_stroke);
    let painter = ui.painter();
    if selected {
        painter.rect_stroke(
            thumb.expand(3.0),
            Rounding::same(7.0),
            Stroke::new(2.5_f32, palette.accent),
        );
    } else if response.hovered() {
        painter.rect_stroke(
            thumb.expand(3.0),
            Rounding::same(7.0),
            Stroke::new(1.5_f32, palette.track_off),
        );
    }

    // The theme in use carries a dot before its name, as the wallpaper in use
    // does on a Mac: the ring says which one is being looked at, which after a
    // double-click on another is not the same thing.
    let colour = if active {
        palette.heading
    } else {
        palette.text
    };
    let galley = painter.layout(theme.name.clone(), font, colour, TILE.x - 12.0);
    let dot = if active { 10.0 } else { 0.0 };
    let width = galley.size().x + dot;
    let left = thumb.center().x - width / 2.0;
    let y = thumb.bottom() + 5.0;
    if active {
        painter.circle_filled(
            pos2(left + 3.5, y + galley.size().y / 2.0),
            3.5,
            palette.accent,
        );
    }
    painter
        .with_clip_rect(rect)
        .galley(pos2(left + dot, y), galley, colour);
    response
}

/// A theme in miniature: its title bar with the window buttons in their
/// colours, a few lines of coloured ObjectScript on its background, the
/// cursor and a selection, and the eight normal ANSI colours along the foot.
fn paint_thumbnail(painter: &egui::Painter, rect: Rect, theme: &Theme, outline: Color32) {
    let painter = painter.with_clip_rect(rect);
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 15.0));
    let body = Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max);

    painter.rect_filled(bar, 0.0, theme.ui_background);
    if let Some(gradient) = theme.ui_gradient.as_ref() {
        crate::ui::shading::ui_gradient(&painter, bar, gradient);
    }
    let tab = Rect::from_min_size(bar.min + vec2(6.0, 3.0), vec2(34.0, bar.height() - 3.0));
    let lifted = crate::term::palette::blend(theme.ui_background, theme.ui_foreground, 0.22);
    painter.rect_filled(
        tab,
        Rounding {
            nw: 2.0,
            ne: 2.0,
            sw: 0.0,
            se: 0.0,
        },
        theme.tab_selected.unwrap_or(lifted),
    );
    let style = theme.window_buttons.style;
    let dots = [Slot::Close, Slot::Maximize, Slot::Minimize];
    for (n, slot) in dots.into_iter().enumerate() {
        let colour = match slot {
            Slot::Close => theme.window_buttons.close,
            Slot::Maximize => theme.window_buttons.maximize,
            _ => theme.window_buttons.minimize,
        }
        .or(style.default_colour(slot))
        .unwrap_or(theme.ui_foreground);
        let x = bar.right() - 7.0 - n as f32 * 9.0;
        painter.circle_filled(pos2(x, bar.center().y), 3.0, colour);
    }

    painter.rect_filled(body, 0.0, theme.background);
    if let Some(gradient) = theme.background_gradient.as_ref() {
        crate::ui::shading::ui_gradient(&painter, body, gradient);
    }
    let mut y = body.top() + 4.0;
    for line in &SAMPLE[..4] {
        let galley = painter.layout_job(coloured_line(line, theme, 7.5));
        let height = galley.size().y;
        painter.galley(pos2(body.left() + 5.0, y), galley, theme.foreground);
        y += height;
    }
    let prompt = painter.layout_no_wrap(
        "USER>".to_owned(),
        egui::FontId::monospace(7.5),
        theme.foreground,
    );
    let at = pos2(body.left() + 5.0, y);
    let word = Rect::from_min_size(
        at + vec2(prompt.size().x, 1.0),
        vec2(24.0, prompt.size().y - 2.0),
    );
    painter.galley(at, prompt, theme.foreground);
    painter.rect_filled(word, 0.0, theme.selection);
    painter.rect_filled(
        Rect::from_min_size(word.right_top() + vec2(1.0, 0.0), vec2(4.0, word.height())),
        0.0,
        theme.cursor,
    );

    let strip = 4.0;
    let width = rect.width() / 8.0;
    for slot in 0..8 {
        let cell = Rect::from_min_size(
            pos2(rect.left() + slot as f32 * width, rect.bottom() - strip),
            vec2(width + 0.5, strip),
        );
        painter.rect_filled(cell, 0.0, theme.ansi[slot]);
    }
    painter.rect_stroke(
        rect.shrink(0.5),
        Rounding::same(3.0),
        Stroke::new(1.0_f32, outline),
    );
}

/// Duplicate, the two ways to start a theme from nothing, and Delete - for
/// the theme the pages are on.
fn theme_actions(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let source = c.themes[index].clone();
    let deletable = !source.builtin;
    let (mut delete, mut copy, mut new_from) = (false, false, None);
    // Right to left: Delete ends the row, away from the buttons that make.
    card.buttons(|ui| {
        delete = ui
            .add_enabled(deletable, prefs::button_widget(tr("Delete")))
            .on_hover_text(tr("Only your own themes; the built-ins cannot be deleted."))
            .clicked();
        for (label, dark) in [("New from light", false), ("New from dark", true)] {
            if prefs::button(ui, tr(label)).clicked() {
                new_from = Some(dark);
            }
        }
        // Duplicating is the only way to edit a built-in, so it is the first
        // button and it acts on the theme the pages are on.
        copy = prefs::button(ui, tr("Duplicate"))
            .on_hover_text(tr1("A copy of {} that you can edit.", &source.name))
            .clicked();
    });
    if delete {
        c.state.themes.confirm_delete = Some(source.name.clone());
    }

    if let Some(name) = c.state.themes.confirm_delete.clone() {
        let question = tr1("Delete \"{}\"?", &name);
        let spec = Row::new(&question).subtitle(tr("Its colours cannot be recovered."));
        let (mut keep, mut confirmed) = (false, false);
        card.row(spec, |ui| {
            confirmed = prefs::button(ui, tr("Delete")).clicked();
            keep = prefs::button(ui, tr("Keep")).clicked();
        });
        if keep {
            c.state.themes.confirm_delete = None;
        }
        if confirmed {
            c.requests
                .push(UiRequest::Theme(ThemeAction::Delete(name.clone())));
            c.state.themes.confirm_delete = None;
            c.state.themes.selected = None;
        }
    }

    let made = if copy {
        Some(duplicate(
            c.themes,
            &source,
            &format!("{} copy", source.name),
        ))
    } else {
        new_from.map(|dark| {
            // Built from a built-in rather than from nothing: a theme with
            // sixteen unset ANSI slots is not a starting point anyone wants.
            let base = c
                .themes
                .iter()
                .find(|t| t.builtin && t.dark == dark)
                .cloned()
                .unwrap_or_default();
            duplicate(c.themes, &base, "New theme")
        })
    };
    if let Some(name) = made {
        made_theme(c, name);
    }
}

/// A theme just added: the pages move onto it, and it is written and used.
fn made_theme(c: &mut Ctx<'_>, name: String) {
    c.state.themes.selected = Some(name.clone());
    c.state.themes.rename = None;
    c.requests
        .push(UiRequest::Theme(ThemeAction::Save(name.clone())));
    c.requests
        .push(UiRequest::Theme(ThemeAction::Activate(name)));
}

fn themes_folder(ui: &mut Ui, c: &mut Ctx<'_>) {
    if prefs::button(ui, tr("Open folder"))
        .on_hover_text(crate::config::themes_dir().display().to_string())
        .clicked()
    {
        c.requests
            .push(UiRequest::OpenFolder(crate::config::themes_dir()));
    }
}

/// A name no theme in `themes` already answers to, based on `name`.
fn free_name(themes: &[Theme], name: &str) -> String {
    let taken = |candidate: &str| themes.iter().any(|t| t.name == candidate);
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|n| format!("{name} {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

/// Adds a copy of `source` under a free name and returns that name.
fn duplicate(themes: &mut Vec<Theme>, source: &Theme, name: &str) -> String {
    let name = free_name(themes, name);
    let mut copy = source.clone();
    copy.name = name.clone();
    // A copy is the user's, wherever it came from: nothing about it is the
    // built-in any more, least of all the file it would be written to.
    copy.builtin = false;
    copy.path = None;
    themes.push(copy);
    name
}

// ---------------------------------------------------------------------------
// The editor
// ---------------------------------------------------------------------------

fn editor() -> Vec<Section> {
    let link = |key, title, sub| Item::link(key, title, Route::sub(Category::Themes, sub));
    vec![
        untitled(vec![
            Item::rows("theme_name", "Name", name_row)
                .contextual()
                .keys(&["rename", "renomear", "nome"]),
            Item::rows("theme_use", "Use this theme", use_row)
                .contextual()
                .keys(&["apply", "aplicar", "active", "ativo", "duplicate", "duplicar"]),
        ]),
        section(
            "Colours",
            vec![
                link("theme_terminal_link", "Terminal colours", "theme_terminal")
                    .keys(&["background", "foreground", "cursor", "selection", "fundo", "selecao"]),
                link("theme_scrollbar_link", "Scrollbar", "theme_scrollbar")
                    .keys(&["handle", "track", "barra de rolagem"]),
                link("theme_ansi_link", "ANSI", "theme_ansi")
                    .keys(&["16", "bright", "palette", "paleta"]),
                link("theme_chrome_link", "Chrome", "theme_chrome")
                    .sub("The tab strip, the panels and the dialogs - the frame around the terminal rather than the terminal itself.")
                    .keys(&["tab", "aba", "border", "borda", "painel"]),
                link("theme_buttons_link", "Window buttons", "theme_buttons")
                    .keys(&["aqua", "luna", "materia", "close", "minimize", "fechar"]),
                link("theme_order_link", "Title bar buttons, left to right", "theme_order")
                    .keys(&["order", "drag", "ordem", "arrastar"]),
                link("theme_syntax_link", "ObjectScript syntax", "theme_syntax")
                    .keys(&["highlighting", "realce", "sintaxe", "global"]),
            ],
        ),
    ]
}

/// The name, editable on a theme of the user's and only shown on a built-in.
fn name_row(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let name = c.themes[index].name.clone();
    if c.themes[index].builtin {
        let spec = Row::new(tr("Name")).subtitle(tr("Duplicate it to change anything - a built-in is the same in every install, which is what makes it something to fall back to."));
        card.row(spec, |ui| {
            ui.strong(&name);
            ui.weak(tr("built-in, read-only"));
        });
        return;
    }

    let saved_in = c.themes[index]
        .path
        .as_ref()
        .map(|path| tr1("Saved in {}", &path.display().to_string()));
    let state = &mut c.state.themes;
    let (owner, mut draft) = state
        .rename
        .get_or_insert_with(|| (name.clone(), name.clone()))
        .clone();
    // The field belongs to the theme it was opened on: moving to another one
    // while typing must not carry the half-typed name over to it, nor rename
    // the one that was left behind.
    if owner != name {
        draft = name.clone();
    }
    let themes = &*c.themes;
    let mut commit = false;
    let mut taken = false;
    card.row(Row::new(tr("Name")).subtitle(saved_in.as_deref()), |ui| {
        let response = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(220.0));
        taken = themes
            .iter()
            .any(|t| t.name == draft.trim() && t.name != name);
        if taken {
            ui.colored_label(WARNING, tr("Already in use"));
        }
        // Committed on Enter or on leaving the field, so every keystroke is
        // not a rename - and never to a name that is taken, which would
        // shadow another theme in the picker.
        commit = response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    let valid = !draft.trim().is_empty() && !taken;
    state.rename = Some((name.clone(), draft.clone()));
    if commit && valid && draft.trim() != name {
        let renamed = draft.trim().to_string();
        c.themes[index].name = renamed.clone();
        state.selected = Some(renamed.clone());
        state.rename = None;
        c.requests
            .push(UiRequest::Theme(ThemeAction::Save(renamed.clone())));
        if name == c.settings.theme {
            c.requests
                .push(UiRequest::Theme(ThemeAction::Activate(renamed)));
        }
    }
}

/// Using the theme, and copying it.
fn use_row(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let source = c.themes[index].clone();
    let in_use = source.name == c.settings.theme;
    let (mut apply, mut copy) = (false, false);
    card.buttons(|ui| {
        copy = prefs::button(ui, tr("Duplicate"))
            .on_hover_text(tr1("A copy of {} that you can edit.", &source.name))
            .clicked();
        if in_use {
            ui.weak(tr("In use"));
        } else {
            apply = prefs::button(ui, tr("Use this theme")).clicked();
        }
    });
    if apply {
        c.requests
            .push(UiRequest::Theme(ThemeAction::Activate(source.name.clone())));
    }
    if copy {
        let name = duplicate(c.themes, &source, &format!("{} copy", source.name));
        made_theme(c, name);
    }
}

// ---------------------------------------------------------------------------
// The colour groups
// ---------------------------------------------------------------------------

/// The grid itself.
fn terminal_colours() -> Vec<Section> {
    vec![
        untitled(vec![
            Item::colour("th_background", "Background", |t| Some(&mut t.background))
                .keys(&["fundo", "terminal"]),
            Item::colour("th_foreground", "Foreground", |t| Some(&mut t.foreground))
                .keys(&["text", "texto"]),
            Item::colour("th_cursor", "Cursor", |t| Some(&mut t.cursor)),
            Item::colour("th_selection", "Selection", |t| Some(&mut t.selection))
                .keys(&["selecao", "highlight"]),
        ]),
        section(
            "Gradient",
            vec![
                Item::control("th_gradient", "Gradient", |ui, c| {
                    gradient_control(ui, c, |t| &mut t.background_gradient, |t| t.background)
                })
                .contextual()
                .keys(&["degrade", "terminal"]),
                Item::colour("th_gradient_from", "From", |t| {
                    t.background_gradient.as_mut().map(|g| &mut g.from)
                })
                .when(|c| editing_has(c, |t| t.background_gradient.is_some()))
                .keys(&["gradient", "degrade"]),
                Item::colour("th_gradient_to", "To", |t| {
                    t.background_gradient.as_mut().map(|g| &mut g.to)
                })
                .when(|c| editing_has(c, |t| t.background_gradient.is_some()))
                .keys(&["gradient", "degrade"]),
            ],
        )
        .footer("Painted behind the terminal's text in place of the flat background. Text IRIS gave a background colour of its own keeps it."),
    ]
}

/// The terminal's scroll bar: the handle, the strip it runs in, and a
/// gradient along both.
fn scrollbar_colours() -> Vec<Section> {
    use crate::ui::terminal_view::{gradient_end, handle_colour, track_colour};
    vec![untitled(vec![
        Item::fallback("th_handle", "Handle", |t| &mut t.scrollbar_handle, handle_colour)
            .keys(&["scrollbar", "barra de rolagem"]),
        Item::fallback("th_track", "Track", |t| &mut t.scrollbar_track, track_colour)
            .keys(&["scrollbar", "barra de rolagem", "trilho"]),
        Item::control("th_scroll_gradient", "Gradient", scrollbar_gradient)
            .contextual()
            .keys(&["scrollbar", "degrade"]),
        Item::fallback(
            "th_handle_to",
            "Handle to",
            |t| &mut t.scrollbar_handle_to,
            |t| gradient_end(handle_colour(t)),
        )
        .when(scrollbar_has_gradient)
        .keys(&["scrollbar", "gradient", "degrade"]),
        Item::fallback(
            "th_track_to",
            "Track to",
            |t| &mut t.scrollbar_track_to,
            |t| gradient_end(track_colour(t)),
        )
        .when(scrollbar_has_gradient)
        .keys(&["scrollbar", "gradient", "degrade"]),
    ])
    .footer("Read as the vertical bar sees it: vertical runs along the bar, horizontal across it. The bar along the bottom is turned to match.")]
}

fn scrollbar_has_gradient(c: &Ctx<'_>) -> bool {
    editing_has(c, |t| t.scrollbar_gradient.is_some())
}

/// The sixteen IRIS asks for by number.
fn ansi_colours() -> Vec<Section> {
    vec![
        untitled(vec![Item::rows("th_ansi", "ANSI colours", ansi_rows)
            .contextual()
            .keys(&[
                "16", "bright", "palette", "paleta", "black", "red", "green",
            ])])
        .footer("0-7 normal, 8-15 bright: the sixteen colours IRIS can ask for by number."),
    ]
}

/// The frame round the terminal: tab strip, panels, dialogs.
fn chrome_colours() -> Vec<Section> {
    vec![
        untitled(vec![
            Item::colour("th_ui_background", "Background", |t| Some(&mut t.ui_background))
                .keys(&["chrome", "fundo", "interface"]),
            Item::colour("th_ui_text", "Text", |t| Some(&mut t.ui_foreground))
                .keys(&["chrome", "texto", "interface"]),
            Item::fallback("th_border", "Border", |t| &mut t.ui_border, |t| t.ui_foreground)
                .keys(&["chrome", "borda"]),
            // What an unset one looks like is the widget colours', which lift
            // the chrome background towards its text.
            Item::fallback("th_tab", "Selected tab", |t| &mut t.tab_selected, |t| {
                crate::term::palette::blend(t.ui_background, t.ui_foreground, 0.22)
            })
            .keys(&["tab", "aba"]),
            Item::fallback(
                "th_tab_text",
                "Selected tab text",
                |t| &mut t.tab_selected_text,
                |t| t.ui_foreground,
            )
            .keys(&["tab", "aba", "texto"]),
        ])
        .footer("The tab strip, the panels and the dialogs - the frame around the terminal rather than the terminal itself."),
        section(
            "Gradient",
            vec![
                Item::control("th_ui_gradient", "Gradient", |ui, c| {
                    gradient_control(ui, c, |t| &mut t.ui_gradient, |t| t.ui_background)
                })
                .contextual()
                .keys(&["chrome", "degrade"]),
                Item::colour("th_ui_from", "From", |t| t.ui_gradient.as_mut().map(|g| &mut g.from))
                    .when(|c| editing_has(c, |t| t.ui_gradient.is_some()))
                    .keys(&["chrome", "gradient", "degrade"]),
                Item::colour("th_ui_to", "To", |t| t.ui_gradient.as_mut().map(|g| &mut g.to))
                    .when(|c| editing_has(c, |t| t.ui_gradient.is_some()))
                    .keys(&["chrome", "gradient", "degrade"]),
            ],
        )
        .footer("Painted behind the title bar, the tab strip and the dialogs in place of the flat background. Menus keep the flat one, so they stay readable over the terminal."),
    ]
}

/// What a window button's colour falls back to when the theme does not set
/// it, which is what the swatch has to show rather than a black hole.
fn slot_colour(theme: &Theme, slot: Slot) -> Color32 {
    theme
        .window_buttons
        .style
        .default_colour(slot)
        .unwrap_or(theme.ui_foreground)
}

/// What the gear and the `+` fall back to: the glyph colour when the theme
/// names one, and the interface foreground otherwise.
fn icon_colour(theme: &Theme) -> Color32 {
    theme.window_buttons.icon.unwrap_or(theme.ui_foreground)
}

/// The title bar's controls: how they are drawn and their colours.
fn button_colours() -> Vec<Section> {
    vec![
        untitled(vec![Item::control(
            "th_button_style",
            "Style",
            button_style,
        )
        .contextual()
        .keys(&["aqua", "luna", "materia", "stroked", "estilo"])])
        .footer("Which buttons are shown is set per button in Title bar buttons, left to right."),
        section(
            "Colours",
            vec![
                Item::fallback(
                    "th_close",
                    "Close",
                    |t| &mut t.window_buttons.close,
                    |t| slot_colour(t, Slot::Close),
                )
                .keys(&["fechar", "button", "botao"]),
                Item::fallback(
                    "th_minimize",
                    "Minimize",
                    |t| &mut t.window_buttons.minimize,
                    |t| slot_colour(t, Slot::Minimize),
                )
                .keys(&["minimizar", "button", "botao"]),
                Item::fallback(
                    "th_maximize",
                    "Maximize",
                    |t| &mut t.window_buttons.maximize,
                    |t| slot_colour(t, Slot::Maximize),
                )
                .keys(&["maximizar", "button", "botao"]),
                Item::fallback(
                    "th_glyph",
                    "Glyph",
                    |t| &mut t.window_buttons.icon,
                    |t| t.ui_foreground,
                )
                .keys(&["icon", "icone", "simbolo"]),
                Item::fallback(
                    "th_close_hover",
                    "Close hover",
                    |t| &mut t.window_buttons.hover_close,
                    |t| t.ui_foreground,
                )
                .keys(&["fechar", "hover"]),
                // The two marks that are not window controls, and the two
                // people actually aim at every day. Unset, both follow the
                // glyph colour above.
                Item::fallback(
                    "th_gear",
                    "Settings gear",
                    |t| &mut t.window_buttons.settings,
                    icon_colour,
                )
                .keys(&["engrenagem", "configuracoes"]),
                Item::fallback(
                    "th_new_tab",
                    "New tab +",
                    |t| &mut t.window_buttons.new_tab,
                    icon_colour,
                )
                .keys(&["nova aba", "plus", "mais"]),
                Item::fallback(
                    "th_on_top",
                    "Always on top",
                    |t| &mut t.window_buttons.on_top,
                    |t| t.window_buttons.settings.unwrap_or(icon_colour(t)),
                )
                .keys(&["pin", "fixar", "sempre visivel"]),
                Item::fallback(
                    "th_close_tab",
                    "Tab close x",
                    |t| &mut t.window_buttons.close_tab,
                    |t| match t.window_buttons.style {
                        WindowButtonStyle::Stroke => t.ui_foreground,
                        _ => t.window_buttons.close.unwrap_or(t.ui_foreground),
                    },
                )
                .keys(&["tab", "aba", "fechar"]),
            ],
        ),
    ]
}

fn title_bar_order() -> Vec<Section> {
    vec![untitled(vec![Item::rows(
        "th_order",
        "Title bar buttons, left to right",
        order_rows,
    )
    .contextual()
    .keys(&["order", "drag", "reorder", "ordem", "arrastar", "reordenar", "left space", "right space"])])
    .footer("Drag a row by its grip, or move it with the arrows. A button switched off keeps its place, and comes back to it when it is switched on again.")]
}

/// The highlighting of ObjectScript at the prompt.
fn syntax_colours() -> Vec<Section> {
    let keys: &'static [&'static str] = &[
        "syntax",
        "sintaxe",
        "objectscript",
        "highlighting",
        "realce",
    ];
    let row = |key, title, get: fn(&mut Theme) -> Option<&mut Color32>| {
        Item::colour(key, title, get).keys(keys)
    };
    vec![untitled(vec![
        row("th_syn_label", "Label", |t| Some(&mut t.syntax_label)),
        row("th_syn_command", "Command", |t| Some(&mut t.syntax_command)),
        row("th_syn_string", "String", |t| Some(&mut t.syntax_string)),
        row("th_syn_number", "Number", |t| Some(&mut t.syntax_number)),
        row("th_syn_delimiter", "Delimiter", |t| Some(&mut t.syntax_delimiter)),
        row("th_syn_operator", "Operator", |t| Some(&mut t.syntax_operator)),
        row("th_syn_preprocessor", "Preprocessor", |t| Some(&mut t.syntax_preprocessor)),
        row("th_syn_function", "Function", |t| Some(&mut t.syntax_function)),
        row("th_syn_global", "Global", |t| Some(&mut t.syntax_global)),
        row("th_syn_system", "System variable", |t| Some(&mut t.syntax_system_variable)),
        row("th_syn_class", "Class", |t| Some(&mut t.syntax_class)),
        row("th_syn_method", "Method", |t| Some(&mut t.syntax_method)),
        row("th_syn_attribute", "Attribute", |t| Some(&mut t.syntax_attribute)),
        row("th_syn_member", "Member", |t| Some(&mut t.syntax_member)),
        row("th_syn_routine", "Routine", |t| Some(&mut t.syntax_routine)),
        row("th_syn_extrinsic", "Extrinsic", |t| Some(&mut t.syntax_extrinsic)),
    ])
    .footer("Named after the semantic token scopes of the InterSystems VS Code extension, so an editor colour customisation can be copied across field by field.")]
}

// ---------------------------------------------------------------------------
// The rows' controls
// ---------------------------------------------------------------------------

/// A `Kind::Colour` row. `None` when there is no theme to edit.
pub(super) fn colour_item(
    card: &mut Card<'_>,
    spec: Row<'_>,
    c: &mut Ctx<'_>,
    get: fn(&mut Theme) -> Option<&mut Color32>,
) -> Option<RowResponse<()>> {
    let index = editing(c)?;
    let editable = !c.themes[index].builtin;
    let theme = &mut c.themes[index];
    let mut changed = false;
    let shown = card.row(spec, |ui| {
        if let Some(value) = get(theme) {
            changed = ui
                .add_enabled_ui(editable, |ui| swatch(ui, value, editable))
                .inner;
        }
    });
    c.theme_edited |= changed;
    Some(shown)
}

/// A `Kind::Fallback` row: the swatch shows the colour that will
/// actually be painted, and the reset button beside it is what gives one
/// back to the style. There used to be a checkbox here meaning "is this set",
/// which read as if it turned the button off and did nothing of the sort.
pub(super) fn fallback_item(
    card: &mut Card<'_>,
    spec: Row<'_>,
    c: &mut Ctx<'_>,
    get: fn(&mut Theme) -> &mut Option<Color32>,
    or: fn(&Theme) -> Color32,
) -> Option<RowResponse<()>> {
    let index = editing(c)?;
    let editable = !c.themes[index].builtin;
    let theme = &mut c.themes[index];
    let fallback = or(theme);
    let value = get(theme);
    let mut changed = false;
    let shown = card.row(spec, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            let mut colour = value.unwrap_or(fallback);
            if swatch(ui, &mut colour, editable) {
                *value = Some(colour);
                changed = true;
            }
            // Only offered when there is something to go back from.
            if value.is_some()
                && ui
                    .small_button("\u{21ba}")
                    .on_hover_text(tr("Back to the colour the button style supplies."))
                    .clicked()
            {
                *value = None;
                changed = true;
            }
        });
    });
    c.theme_edited |= changed;
    Some(shown)
}

/// A colour button that can also be copied from and pasted into. Reports a
/// change.
///
/// Copy and paste work two ways. Pointing at a swatch, Ctrl+C puts its hex
/// code on the clipboard and Ctrl+V takes one off it - from anywhere, so a
/// colour can come from an editor theme as well as from another swatch. The
/// right-click menu does the same for whoever does not know the keys; its
/// Paste offers the last colour copied here, because egui can only read the
/// system clipboard as the paste event of a key press, not on demand.
///
/// Copying is offered on a read-only theme too: taking a colour out of a
/// built-in is the most common reason to want one.
fn swatch(ui: &mut Ui, value: &mut Color32, editable: bool) -> bool {
    let response = color_edit_button_srgba(ui, value, Alpha::Opaque);
    let mut changed = response.changed();
    if let Some(pasted) = colour_clipboard(ui, &response, *value, editable) {
        if pasted != *value {
            *value = pasted;
            changed = true;
        }
    }
    changed
}

fn clipboard_id() -> egui::Id {
    egui::Id::new("nit-theme-colour-clipboard")
}

/// Copy and paste for one swatch: see [`swatch`]. Returns a pasted colour.
fn colour_clipboard(
    ui: &Ui,
    response: &egui::Response,
    current: Color32,
    editable: bool,
) -> Option<Color32> {
    let hex = crate::config::theme::to_hex(current);
    let copy = |ui: &Ui| {
        ui.ctx().copy_text(hex.clone());
        ui.data_mut(|d| d.insert_temp(clipboard_id(), current));
        notice(ui, tr1("Copied {}", &hex));
    };
    let mut pasted = None;
    // Asked of the pointer and the rectangle rather than of the widget: a
    // disabled swatch (on a built-in) is never hovered, and the row's own
    // click area over the swatch can take the hover from it, which left the
    // keys doing nothing while the swatch still looked pointed at.
    if ui.rect_contains_pointer(response.rect) {
        let (copied, paste) = ui.input(|i| {
            let copied = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let paste = i.events.iter().find_map(|e| match e {
                egui::Event::Paste(text) => crate::config::theme::parse_hex(text),
                _ => None,
            });
            (copied, paste)
        });
        if copied {
            copy(ui);
        }
        if let Some(colour) = paste {
            if editable {
                pasted = Some(colour);
                notice(ui, tr1("Pasted {}", &crate::config::theme::to_hex(colour)));
            } else {
                notice(
                    ui,
                    tr("A built-in theme cannot be changed; duplicate it first.").to_string(),
                );
            }
        }
    }
    show_notice(ui, response);
    let stored: Option<Color32> = ui.data(|d| d.get_temp(clipboard_id()));
    response.context_menu(|ui| {
        if ui.button(tr1("Copy {}", &hex)).clicked() {
            copy(ui);
            ui.close_menu();
        }
        let paste = ui.add_enabled(
            editable && stored.is_some(),
            egui::Button::new(match stored {
                Some(colour) => tr1("Paste {}", &crate::config::theme::to_hex(colour)),
                None => tr("Paste").to_string(),
            }),
        );
        if paste.clicked() {
            pasted = stored;
            ui.close_menu();
        }
        ui.weak(tr("Ctrl+C / Ctrl+V while pointing at a colour"));
    });
    pasted
}

fn notice_id() -> egui::Id {
    egui::Id::new("nit-theme-colour-notice")
}

/// Says what a copy or paste just did. The keys work without the pointer
/// leaving the swatch and without anything visibly moving, so without this a
/// copy that worked and one that did not looked exactly the same.
fn notice(ui: &Ui, text: String) {
    let until = ui.input(|i| i.time) + 1.5;
    ui.data_mut(|d| d.insert_temp(notice_id(), (text, until, ui.next_auto_id())));
}

/// The notice beside the swatch it is about, while it lasts.
fn show_notice(ui: &Ui, response: &egui::Response) {
    let Some((text, until, _)) = ui.data(|d| d.get_temp::<(String, f64, egui::Id)>(notice_id()))
    else {
        return;
    };
    let now = ui.input(|i| i.time);
    if now > until || !ui.rect_contains_pointer(response.rect) {
        return;
    }
    egui::show_tooltip_at(
        ui.ctx(),
        ui.layer_id(),
        notice_id(),
        response.rect.left_bottom(),
        |ui| {
            ui.label(text);
        },
    );
    // Asked for once, for the moment it should disappear: nothing else would
    // draw the frame that takes it down.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64((until - now).max(0.0)));
}

/// Which way a gradient runs, or `None` for a flat fill. Reports a change.
fn direction_picker(ui: &mut Ui, direction: &mut Option<GradientDirection>) -> bool {
    let options: Vec<(Option<GradientDirection>, &str)> = std::iter::once((None, tr("None")))
        .chain(GradientDirection::ALL.iter().map(|&choice| {
            let label = match choice {
                GradientDirection::Vertical => "Vertical",
                GradientDirection::Horizontal => "Horizontal",
                GradientDirection::Diagonal => "Diagonal",
            };
            (Some(choice), tr(label))
        }))
        .collect();
    prefs::segmented(ui, direction, &options)
}

/// A gradient in place of a flat fill: none, or which way it runs. `flat` is
/// the fill it replaces; the two ends are rows of their own.
fn gradient_control(
    ui: &mut Ui,
    c: &mut Ctx<'_>,
    get: fn(&mut Theme) -> &mut Option<UiGradient>,
    flat: fn(&Theme) -> Color32,
) {
    let Some(index) = editing(c) else {
        return;
    };
    let editable = !c.themes[index].builtin;
    let theme = &mut c.themes[index];
    let flat = flat(theme);
    let gradient = get(theme);
    let mut direction = gradient.map(|g| g.direction);
    let changed = ui
        .add_enabled_ui(editable, |ui| direction_picker(ui, &mut direction))
        .inner;
    if changed {
        *gradient = direction.map(|direction| {
            // Switching it on starts from the flat fill lifted and lowered a
            // little, so the first click already shows a gradient rather than
            // two equal ends.
            let (from, to) = gradient.map(|g| (g.from, g.to)).unwrap_or((
                crate::ui::shading::lighten(flat, 0.25),
                crate::ui::shading::darken(flat, 0.55),
            ));
            UiGradient {
                direction,
                from,
                to,
            }
        });
        c.theme_edited = true;
    }
}

fn scrollbar_gradient(ui: &mut Ui, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let editable = !c.themes[index].builtin;
    let direction = &mut c.themes[index].scrollbar_gradient;
    c.theme_edited |= ui
        .add_enabled_ui(editable, |ui| direction_picker(ui, direction))
        .inner;
}

fn button_style(ui: &mut Ui, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let editable = !c.themes[index].builtin;
    let options: Vec<(WindowButtonStyle, &str)> = WindowButtonStyle::ALL
        .iter()
        .map(|&style| {
            let label = match style {
                WindowButtonStyle::Stroke => "Stroked",
                WindowButtonStyle::Aqua => "Aqua",
                WindowButtonStyle::Luna => "Luna",
                WindowButtonStyle::Materia => "Materia",
            };
            (style, tr(label))
        })
        .collect();
    let style = &mut c.themes[index].window_buttons.style;
    c.theme_edited |= ui
        .add_enabled_ui(editable, |ui| prefs::segmented(ui, style, &options))
        .inner;
}

/// The sixteen ANSI slots, in the two rows they are numbered in.
fn ansi_rows(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let editable = !c.themes[index].builtin;
    let theme = &mut c.themes[index];
    let mut changed = false;
    card.custom(|ui| {
        Grid::new("theme-ansi")
            .num_columns(8)
            .spacing(vec2(14.0, 8.0))
            .show(ui, |ui| {
                for half in 0..2 {
                    for slot in 0..8 {
                        let at = half * 8 + slot;
                        ui.vertical(|ui| {
                            ui.small(format!("{at}"));
                            changed |= ui
                                .add_enabled_ui(editable, |ui| {
                                    swatch(ui, &mut theme.ansi[at], editable)
                                })
                                .inner;
                        });
                    }
                    ui.end_row();
                }
            });
    });
    c.theme_edited |= changed;
}

/// Which title-bar buttons a theme shows, and in what order.
///
/// One list for both, so a button is switched off where it is moved: hiding
/// it keeps its place, and switching it back on returns it there. Each row is
/// dragged by the grip at its left; the arrows stay for whoever would rather
/// not aim.
fn order_rows(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        return;
    };
    let editable = !c.themes[index].builtin;
    let buttons = &mut c.themes[index].window_buttons;
    let mut changed = false;
    let mut moved = None;
    let last = buttons.order.len() - 1;
    card.custom(|ui| {
        let dragging = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|from| *from);
        ui.add_enabled_ui(editable, |ui| {
            Grid::new("theme-button-order")
                .num_columns(3)
                .spacing(vec2(12.0, 6.0))
                .show(ui, |ui| {
                    for (at, button) in buttons.order.into_iter().enumerate() {
                        let label = match button {
                            TitleButton::Close => tr("Close"),
                            TitleButton::Minimize => tr("Minimize"),
                            TitleButton::Maximize => tr("Maximize"),
                            TitleButton::OnTop => tr("Always on top"),
                            TitleButton::Settings => tr("Settings gear"),
                            TitleButton::NewTab => tr("New tab +"),
                            TitleButton::Tabs => tr("Tabs"),
                            TitleButton::LeftSpace => tr("Left space"),
                            TitleButton::RightSpace => tr("Right space"),
                        };
                        let grip = if editable {
                            ui.dnd_drag_source(egui::Id::new(("theme-order-grip", at)), at, grip)
                                .response
                                .on_hover_text(tr("Drag to move"))
                        } else {
                            grip(ui)
                        };
                        let shown = match button {
                            TitleButton::Close => Some(&mut buttons.show_close),
                            TitleButton::Minimize => Some(&mut buttons.show_minimize),
                            TitleButton::Maximize => Some(&mut buttons.show_maximize),
                            TitleButton::OnTop => Some(&mut buttons.show_on_top),
                            // Placed, never hidden: the gear is the only way
                            // into Settings, the + the only way to the server
                            // menu, and the spaces are where the bar drags.
                            _ => None,
                        };
                        let name = match shown {
                            Some(shown) => {
                                let response = ui.checkbox(shown, label);
                                changed |= response.changed();
                                response
                            }
                            None if button.is_space() => ui
                                .label(egui::RichText::new(label).italics())
                                .on_hover_text(tr(
                                    "Empty title bar, which drags the window. Whatever is before the left space packs against the left-hand end, whatever is after the right space against the right-hand end, and anything between the two is centred.",
                                )),
                            None => ui
                                .add_enabled(false, egui::Checkbox::new(&mut true, label))
                                .on_disabled_hover_text(tr("Always shown.")),
                        };
                        let arrows = ui
                            .horizontal(|ui| {
                                if ui
                                    .add_enabled(at > 0, egui::Button::new("\u{2b05}").small())
                                    .on_hover_text(tr("Move left"))
                                    .clicked()
                                {
                                    moved = Some((at, at - 1));
                                }
                                if ui
                                    .add_enabled(at < last, egui::Button::new("\u{27a1}").small())
                                    .on_hover_text(tr("Move right"))
                                    .clicked()
                                {
                                    moved = Some((at, at + 1));
                                }
                            })
                            .response;
                        ui.end_row();

                        // The whole row is where a dragged one can land, not
                        // only the grip: aiming a drop at a mark the size of a
                        // letter is what dragging was meant to save.
                        let row = grip.rect.union(name.rect).union(arrows.rect);
                        let target = ui.interact(
                            row,
                            egui::Id::new(("theme-order-row", at)),
                            Sense::hover(),
                        );
                        if let Some(from) = dragging.filter(|from| *from != at) {
                            if target.contains_pointer() {
                                // Where it will land: above this row when it
                                // comes from below, below it when it comes from
                                // above.
                                let y = if from > at { row.top() } else { row.bottom() };
                                ui.painter().hline(
                                    row.x_range(),
                                    y,
                                    Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
                                );
                            }
                        }
                        if let Some(from) = target.dnd_release_payload::<usize>() {
                            moved = Some((*from, at));
                        }
                    }
                });
        });
    });
    let mut usual = false;
    card.buttons(|ui| {
        usual = ui
            .add_enabled(editable, prefs::button_widget(tr("Usual order")))
            .clicked();
    });
    if usual {
        buttons.order = crate::config::theme::default_order(false);
        changed = true;
    }
    if let Some((from, to)) = moved {
        let before = buttons.order;
        crate::config::theme::move_in_order(&mut buttons.order, from, to);
        changed |= buttons.order != before;
    }
    c.theme_edited |= changed;
}

/// The mark a row is dragged by: three short rules, drawn rather than set in
/// text, for the same reason the title-bar glyphs are - a font without the
/// character would show a box.
fn grip(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
    let colour = ui.visuals().widgets.inactive.fg_stroke.color;
    let across = rect.shrink2(vec2(2.0, 0.0)).x_range();
    for step in [-4.0, 0.0, 4.0] {
        ui.painter()
            .hline(across, rect.center().y + step, Stroke::new(1.5_f32, colour));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    response
}

// ---------------------------------------------------------------------------
// The preview
// ---------------------------------------------------------------------------

/// The preview above every Themes page: the theme the pages are on, drawn the
/// way the app would draw it.
///
/// A sub-page with no theme to show - the last one was deleted from under it
/// - goes back to the gallery rather than drawing an editor for nothing.
fn preview_top(ui: &mut Ui, c: &mut Ctx<'_>) {
    let Some(index) = editing(c) else {
        if c.state.settings_route.sub.is_some() {
            c.go = Some(Route::page(Category::Themes));
        } else {
            prefs::empty_state(ui, tr("No themes."));
        }
        return;
    };
    ui.add_space(4.0);
    let theme = &c.themes[index];
    prefs::card(ui, "theme-preview", |card| {
        card.custom(|ui| preview(ui, theme));
    });
    ui.add_space(4.0);
}

/// The lines the preview shows.
///
/// Chosen to hit every token kind the scanner knows: a global, a string with a
/// `^` inside it (which is a piece delimiter, not a global), a class/method
/// call, a macro, a system variable, a routine call, a label, and an error line.
const SAMPLE: &[&str] = &[
    "USER>set ^ABC(1,\"item\")=\"a^b^c\"",
    "USER>write $piece(x,\"^\",2),!,$horolog",
    "USER>do ##class(Utils.Base).Run(.args,42)",
    "USER>d $$Tag^CCPV005 ; $$$OK",
    "Label if obj.Prop=1 set obj.Count=obj.Count+1",
    "<UNDEFINED>zRun+7^Utils.Base.1 *args",
];

/// A sample of the theme, painted the way the terminal paints it.
///
/// The point of it: a hex value in a swatch says nothing about whether a
/// global is readable against the background. The lines go through the real
/// scanner, so what is coloured here is exactly what would be coloured in the
/// session.
fn preview(ui: &mut Ui, theme: &Theme) {
    const SIZE: f32 = 12.0;
    ui.spacing_mut().item_spacing.y = 0.0;
    // The chrome: what the tab strip and the window buttons look like, which
    // no swatch can show.
    egui::Frame::none()
        .fill(theme.ui_background)
        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(gradient) = theme.ui_gradient.as_ref() {
                crate::ui::shading::ui_gradient(
                    ui.painter(),
                    ui.max_rect().expand2(vec2(6.0, 4.0)),
                    gradient,
                );
            }
            crate::ui::chrome::sample_bar(ui, &theme.window_buttons, "USER  100x30");
        });

    // The terminal itself, with the sixteen ANSI slots beside the sample on
    // the same background - and the same gradient - they are drawn against.
    egui::Frame::none()
        .fill(theme.background)
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(gradient) = theme.background_gradient.as_ref() {
                crate::ui::shading::ui_gradient(ui.painter(), ui.max_rect().expand(6.0), gradient);
            }
            ui.horizontal_top(|ui| {
                let ansi_width = 8.0 * 11.0 + 7.0 * 2.0;
                let lines_width = (ui.available_width() - ansi_width - 12.0).max(120.0);
                ui.allocate_ui_with_layout(
                    vec2(lines_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(lines_width);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for line in SAMPLE {
                            ui.add(egui::Label::new(coloured_line(line, theme, SIZE)).truncate());
                        }
                        // A selected word and the cursor, the two colours
                        // nothing else in the sample would show.
                        let mut job = egui::text::LayoutJob::default();
                        let font = egui::FontId::monospace(SIZE);
                        let format = |color, background| egui::TextFormat {
                            font_id: font.clone(),
                            color,
                            background,
                            ..Default::default()
                        };
                        job.append("USER>", 0.0, format(theme.foreground, Color32::TRANSPARENT));
                        job.append("selected", 0.0, format(theme.foreground, theme.selection));
                        job.append("\u{2588}", 0.0, format(theme.cursor, Color32::TRANSPARENT));
                        ui.label(job);
                    },
                );
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr("ANSI"))
                            .small()
                            .color(theme.foreground),
                    );
                    ui.add_space(4.0);
                    for half in 0..2 {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for slot in 0..8 {
                                let (rect, _) =
                                    ui.allocate_exact_size(vec2(11.0, 14.0), Sense::hover());
                                ui.painter()
                                    .rect_filled(rect, 1.0, theme.ansi[half * 8 + slot]);
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
            });
        });
}

/// One sample line, coloured by the terminal's own scanner.
fn coloured_line(line: &str, theme: &Theme, size: f32) -> egui::text::LayoutJob {
    use crate::term::cell::Cell;

    let cells: Vec<Cell> = line
        .chars()
        .map(|ch| Cell {
            ch,
            ..Cell::default()
        })
        .collect();
    let chars: Vec<char> = line.chars().collect();
    let spans = crate::term::syntax::scan(&cells);

    let font = egui::FontId::monospace(size);
    let mut job = egui::text::LayoutJob::default();
    let mut push = |from: usize, to: usize, color: egui::Color32| {
        if from >= to {
            return;
        }
        let text: String = chars[from..to.min(chars.len())].iter().collect();
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };

    // The gaps between spans are output rather than code, so they take the
    // terminal's foreground - exactly as the grid paints them.
    let mut at = 0;
    for span in spans {
        if span.start < at {
            continue;
        }
        push(at, span.start, theme.foreground);
        push(span.start, span.end, theme.syntax_color(span.kind));
        at = span.end;
    }
    push(at, chars.len(), theme.foreground);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn themes() -> Vec<Theme> {
        crate::config::theme::builtin_files()
            .iter()
            .map(|f| Theme::from_file(f).as_builtin())
            .collect()
    }

    /// The preview earns its place by showing the syntax colours, so every
    /// sample line has to be one the scanner finds something in - and none of
    /// them may come out entirely in the plain foreground.
    #[test]
    fn every_preview_line_is_coloured() {
        let theme = Theme::default();
        for line in SAMPLE {
            let job = coloured_line(line, &theme, 12.0);
            let text: String = job
                .sections
                .iter()
                .map(|s| &job.text[s.byte_range.clone()])
                .collect();
            assert_eq!(&text, line, "the line came out changed: {text:?}");
            assert!(
                job.sections
                    .iter()
                    .any(|s| s.format.color != theme.foreground),
                "nothing is highlighted in {line:?}"
            );
        }
    }

    /// A duplicate is the user's: editable, and pointing at no file until one
    /// is written for it.
    #[test]
    fn a_duplicate_is_the_users_own() {
        let mut themes = themes();
        let source = themes[0].clone();
        let name = duplicate(&mut themes, &source, "IRIS Dark copy");

        let copy = themes.iter().find(|t| t.name == name).expect("added");
        assert!(!copy.builtin);
        assert_eq!(copy.path, None);
        assert_eq!(copy.background, source.background);
        assert_eq!(copy.syntax_global, source.syntax_global);
    }

    /// Duplicating twice must not produce two themes with one name: the picker
    /// addresses a theme by name, and the second would be unreachable.
    #[test]
    fn duplicating_twice_gives_two_names() {
        let mut themes = themes();
        let source = themes[0].clone();
        let first = duplicate(&mut themes, &source, "IRIS Dark copy");
        let second = duplicate(&mut themes, &source, "IRIS Dark copy");
        assert_ne!(first, second);
        assert_eq!(second, "IRIS Dark copy 2");
    }

    /// A built-in name is taken, so a new theme cannot land on it either.
    #[test]
    fn a_new_name_never_collides_with_a_builtin() {
        let mut themes = themes();
        let source = themes[0].clone();
        assert_eq!(duplicate(&mut themes, &source, "Tokyo"), "Tokyo 2");
    }

    /// The pages follow the active theme until something else is picked, so
    /// opening Themes shows what is on screen.
    #[test]
    fn the_pages_start_on_the_active_theme() {
        let themes = themes();
        let state = ThemePageState::default();
        assert_eq!(state.showing(&themes, "Tokyo").as_deref(), Some("Tokyo"));

        // A selection that no longer exists - the theme was deleted - falls
        // back rather than leaving the pages empty.
        let state = ThemePageState {
            selected: Some("Gone".into()),
            ..ThemePageState::default()
        };
        assert_eq!(state.showing(&themes, "Light").as_deref(), Some("Light"));
    }

    /// Every group of colours has its own page under the editor, reached from
    /// it, and leading back to it rather than to the gallery.
    #[test]
    fn every_colour_group_is_a_page_under_the_theme_editor() {
        let editor = super::super::find_page(EDITOR).expect("the editor exists");
        assert_eq!(editor.back(), Some(Route::page(Category::Themes)));
        for sub in [
            "theme_terminal",
            "theme_scrollbar",
            "theme_ansi",
            "theme_chrome",
            "theme_buttons",
            "theme_order",
            "theme_syntax",
        ] {
            let route = Route::sub(Category::Themes, sub);
            let group = super::super::find_page(route).expect(sub);
            assert_eq!(group.back(), Some(EDITOR), "{sub}");
            assert!(group.top.is_some(), "{sub} has no preview");
        }
    }
}
