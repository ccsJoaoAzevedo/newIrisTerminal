//! The theme manager: pick a theme, duplicate one, and edit every colour it
//! carries.
//!
//! Themes were TOML files and nothing else, so changing a colour meant finding
//! the folder, editing hex by hand, and restarting. This is the same file
//! format with a front end on it — and the same distinction the loader now
//! makes: the built-ins live in the binary and are immutable, so the first
//! thing anyone does here is duplicate one.
//!
//! Like the other panels this one only decides; `app.rs` does the writing, so a
//! theme cannot be saved or deleted from inside a paint pass.

use egui::color_picker::{color_edit_button_srgba, Alpha};
use egui::{Color32, Context, Grid, Ui};

use crate::config::theme::{Theme, WindowButtonStyle};
use crate::i18n::{tr, tr1};

/// Something the theme manager asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeAction {
    /// Make this theme the active one.
    Activate(String),
    /// Write this theme to its file, creating the file if it has none.
    Save(String),
    /// Forget this theme and delete its file.
    Delete(String),
}

/// State the manager keeps between frames.
#[derive(Default)]
pub struct ThemeManagerState {
    pub open: bool,
    /// Which theme is being edited. `None` follows whatever is active, so
    /// opening the manager lands on the theme currently on screen.
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

impl ThemeManagerState {
    /// The theme the editor should show, given what is active.
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

/// One colour, as a label and a swatch. Returns true when it was changed.
fn colour_row(ui: &mut Ui, label: &'static str, value: &mut Color32, editable: bool) -> bool {
    ui.label(tr(label));
    let changed = ui
        .add_enabled_ui(editable, |ui| {
            color_edit_button_srgba(ui, value, Alpha::Opaque).changed()
        })
        .inner;
    ui.end_row();
    changed
}

/// A colour a theme is allowed to leave unset, where unset means "whatever the
/// button style says" - the Aqua green, the Luna blue, or the widget colours
/// under the stroked style.
///
/// The swatch always shows the colour that will actually be painted, and the
/// reset button beside it is what gives one back to the style. There used to be
/// a checkbox here meaning "is this set", which read as if it turned the button
/// off and did nothing of the sort.
fn optional_colour_row(
    ui: &mut Ui,
    label: &'static str,
    value: &mut Option<Color32>,
    fallback: Color32,
    editable: bool,
    shown: Option<&mut bool>,
) -> bool {
    ui.label(tr(label));
    let changed = ui
        .add_enabled_ui(editable, |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                // Only the three controls can be hidden; the glyph and the
                // hover colour belong to whichever of them is drawn.
                if let Some(shown) = shown {
                    if ui
                        .checkbox(shown, tr("Shown"))
                        .on_hover_text(tr(
                            "Whether the title bar has this control at all. Hiding it takes it out of the row; Ctrl+W and Alt+F4 still close the window.",
                        ))
                        .changed()
                    {
                        changed = true;
                    }
                }
                let mut colour = value.unwrap_or(fallback);
                if color_edit_button_srgba(ui, &mut colour, Alpha::Opaque).changed() {
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
            changed
        })
        .inner;
    ui.end_row();
    changed
}

/// `colour_row` for a run of fields, so sixteen syntax colours are sixteen
/// lines rather than sixteen blocks.
macro_rules! colour_rows {
    ($ui:expr, $theme:expr, $editable:expr, $changed:expr, $(($label:expr, $field:ident)),* $(,)?) => {
        $(
            if colour_row($ui, $label, &mut $theme.$field, $editable) {
                $changed = true;
            }
        )*
    };
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

/// The theme manager window.
///
/// Edits land in `themes` as they are made, so the terminal behind the window
/// repaints in the colour being dragged; the returned actions are what has to
/// reach the disk and the settings.
pub fn theme_manager(
    ctx: &Context,
    state: &mut ThemeManagerState,
    themes: &mut Vec<Theme>,
    active: &str,
    buttons: &crate::config::theme::WindowButtons,
    native_decorations: bool,
) -> Vec<ThemeAction> {
    let mut actions: Vec<ThemeAction> = Vec::new();
    if !state.open {
        return actions;
    }
    let mut open = true;
    let mut changed = false;

    crate::ui::detach::shell(
        ctx,
        "nit-themes",
        tr("Themes"),
        &mut open,
        // Wide enough for all three columns at once: the list, the editor with
        // its hints wrapped, and the preview. The preview is the reason the
        // window is this wide - clipped off the right-hand edge it is worth
        // nothing.
        [1240.0, 700.0],
        buttons,
        native_decorations,
        |ui| {
            let showing = state.showing(themes, active);
            // Resolved once, out here: the editor edits by index and the
            // preview reads the same theme back, and looking it up twice inside
            // the panes would mean two searches that could disagree.
            let index = showing
                .as_deref()
                .and_then(|name| themes.iter().position(|t| t.name == name));
            let Some(index) = index else {
                ui.label(tr("No themes."));
                return;
            };
            ui.horizontal_top(|ui| {
                theme_list(ui, state, themes, active, showing.as_deref(), &mut actions);
                ui.separator();
                ui.vertical(|ui| {
                    // Bounded at both ends: without a maximum the long hints
                    // under each group refuse to wrap and push the preview off
                    // the edge of the window.
                    ui.set_min_width(460.0);
                    ui.set_max_width(560.0);
                    changed = editor(ui, state, themes, index, active, &mut actions);
                });
                ui.separator();
                // Last, so it reads the colours the editor has just changed:
                // the whole point of it is that a dragged swatch shows up here
                // in the same frame.
                ui.vertical(|ui| preview(ui, &themes[index]));
            });
        },
    );

    if changed {
        if let Some(name) = state.showing(themes, active) {
            state.dirty = Some(name);
        }
    }
    // One write per adjustment rather than one per frame of a drag.
    if !ctx.input(|i| i.pointer.any_down()) {
        if let Some(name) = state.dirty.take() {
            actions.push(ThemeAction::Save(name));
        }
    }

    if !open {
        state.open = false;
        // Closing with an edit in flight still saves it: the window is not a
        // dialog with an OK button, and nothing else would ever write it.
        if let Some(name) = state.dirty.take() {
            actions.push(ThemeAction::Save(name));
        }
    }
    actions
}

/// The left-hand pane: the built-ins, the user's own, and the buttons that make
/// a new one.
fn theme_list(
    ui: &mut Ui,
    state: &mut ThemeManagerState,
    themes: &mut Vec<Theme>,
    active: &str,
    showing: Option<&str>,
    actions: &mut Vec<ThemeAction>,
) {
    ui.vertical(|ui| {
        ui.set_min_width(180.0);
        ui.set_max_width(200.0);
        egui::ScrollArea::vertical()
            .id_source("theme-list")
            .max_height(400.0)
            .show(ui, |ui| {
                ui.label(tr("Built-in"));
                for name in names_where(themes, true) {
                    entry(ui, state, &name, showing, active);
                }
                ui.add_space(8.0);
                ui.label(tr("My themes"));
                let mine = names_where(themes, false);
                if mine.is_empty() {
                    ui.weak(tr("None yet - duplicate one."));
                }
                for name in mine {
                    entry(ui, state, &name, showing, active);
                }
            });

        ui.add_space(8.0);
        ui.separator();

        // Duplicating is the only way to edit a built-in, so it is the first
        // button and it acts on whatever is selected.
        let source = showing.and_then(|name| themes.iter().find(|t| t.name == name).cloned());
        if let Some(source) = source {
            if ui
                .button(tr("Duplicate"))
                .on_hover_text(tr1("A copy of {} that you can edit.", &source.name))
                .clicked()
            {
                let name = duplicate(themes, &source, &format!("{} copy", source.name));
                state.selected = Some(name.clone());
                actions.push(ThemeAction::Save(name.clone()));
                actions.push(ThemeAction::Activate(name));
            }
        }
        for (label, base) in [("New from dark", true), ("New from light", false)] {
            if ui.button(tr(label)).clicked() {
                // Built from a built-in rather than from nothing: a theme with
                // sixteen unset ANSI slots is not a starting point anyone wants.
                let source = themes
                    .iter()
                    .find(|t| t.builtin && t.dark == base)
                    .cloned()
                    .unwrap_or_default();
                let name = duplicate(themes, &source, "New theme");
                state.selected = Some(name.clone());
                actions.push(ThemeAction::Save(name.clone()));
                actions.push(ThemeAction::Activate(name));
            }
        }

        let deletable = showing
            .and_then(|name| themes.iter().find(|t| t.name == name))
            .is_some_and(|t| !t.builtin);
        ui.add_enabled_ui(deletable, |ui| {
            if ui
                .button(tr("Delete"))
                .on_hover_text(tr("Only your own themes; the built-ins cannot be deleted."))
                .clicked()
            {
                state.confirm_delete = showing.map(str::to_string);
            }
        });

        if let Some(name) = state.confirm_delete.clone() {
            ui.add_space(4.0);
            ui.label(tr1("Delete \"{}\"?", &name));
            ui.horizontal(|ui| {
                if ui.button(tr("Delete")).clicked() {
                    actions.push(ThemeAction::Delete(name.clone()));
                    state.confirm_delete = None;
                    state.selected = None;
                }
                if ui.button(tr("Keep")).clicked() {
                    state.confirm_delete = None;
                }
            });
        }
    });
}

/// The names of the built-in themes, or of the user's, in list order.
fn names_where(themes: &[Theme], builtin: bool) -> Vec<String> {
    themes
        .iter()
        .filter(|t| t.builtin == builtin)
        .map(|t| t.name.clone())
        .collect()
}

/// One row of the list. Selecting a theme also applies it: a colour scheme is
/// judged against real output, not against a swatch.
fn entry(
    ui: &mut Ui,
    state: &mut ThemeManagerState,
    name: &str,
    showing: Option<&str>,
    active: &str,
) {
    let label = if name == active {
        format!("{name}  *")
    } else {
        name.to_string()
    };
    if ui
        .selectable_label(showing == Some(name), label)
        .on_hover_text(if name == active {
            tr("In use").to_string()
        } else {
            tr1("Show and apply {}", name)
        })
        .clicked()
    {
        state.selected = Some(name.to_string());
        state.confirm_delete = None;
        state.rename = None;
    }
}

/// The right-hand pane. Returns true when a colour was changed.
fn editor(
    ui: &mut Ui,
    state: &mut ThemeManagerState,
    themes: &mut [Theme],
    index: usize,
    active: &str,
    actions: &mut Vec<ThemeAction>,
) -> bool {
    let mut changed = false;
    let builtin = themes[index].builtin;
    let editable = !builtin;
    let name = themes[index].name.clone();

    ui.horizontal(|ui| {
        ui.heading(&name);
        if builtin {
            ui.weak(tr("built-in, read-only"));
        }
        if name != active && ui.button(tr("Apply")).clicked() {
            actions.push(ThemeAction::Activate(name.clone()));
        }
    });
    if builtin {
        ui.small(tr("Duplicate it to change anything - a built-in is the same in every install, which is what makes it something to fall back to."));
    }

    if editable {
        ui.horizontal(|ui| {
            ui.label(tr("Name"));
            let (owner, mut draft) = state
                .rename
                .get_or_insert_with(|| (name.clone(), name.clone()))
                .clone();
            // The field belongs to the theme it was opened on: selecting
            // another one while typing must not carry the half-typed name
            // over to it, nor rename the one that was left behind.
            if owner != name {
                draft = name.clone();
            }
            let response = ui.text_edit_singleline(&mut draft);
            let taken = themes
                .iter()
                .any(|t| t.name == draft.trim() && t.name != name);
            let valid = !draft.trim().is_empty() && !taken;
            state.rename = Some((name.clone(), draft.clone()));
            if taken {
                ui.colored_label(super::panels::WARNING, tr("Already in use"));
            }
            // Committed on Enter or on leaving the field, so every keystroke is
            // not a rename - and never to a name that is taken, which would
            // shadow another theme in the picker.
            let commit = response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if commit && valid && draft.trim() != name {
                let renamed = draft.trim().to_string();
                themes[index].name = renamed.clone();
                state.selected = Some(renamed.clone());
                state.rename = None;
                actions.push(ThemeAction::Save(renamed.clone()));
                if name == active {
                    actions.push(ThemeAction::Activate(renamed));
                }
            }
        });
    }

    egui::ScrollArea::vertical()
        .id_source("theme-editor")
        .show(ui, |ui| {
            let theme = &mut themes[index];

            ui.add_space(4.0);
            ui.strong(tr("Terminal"));
            Grid::new("theme-terminal").num_columns(2).show(ui, |ui| {
                colour_rows!(
                    ui,
                    theme,
                    editable,
                    changed,
                    ("Background", background),
                    ("Foreground", foreground),
                    ("Cursor", cursor),
                    ("Selection", selection),
                );
            });

            ui.add_space(8.0);
            ui.strong(tr("Chrome"));
            ui.small(tr("The tab strip, the panels and the dialogs - the frame around the terminal rather than the terminal itself."));
            Grid::new("theme-chrome").num_columns(2).show(ui, |ui| {
                colour_rows!(
                    ui,
                    theme,
                    editable,
                    changed,
                    ("Background", ui_background),
                    ("Text", ui_foreground),
                );
            });

            ui.add_space(8.0);
            ui.strong(tr("Base"));
            ui.horizontal(|ui| {
                ui.add_enabled_ui(editable, |ui| {
                    for (label, dark) in [("Dark", true), ("Light", false)] {
                        if ui.selectable_label(theme.dark == dark, tr(label)).clicked() {
                            theme.dark = dark;
                            changed = true;
                        }
                    }
                });
                ui.label(tr("Font size"));
                if ui
                    .add_enabled(
                        editable,
                        egui::Slider::new(&mut theme.font_size, 8.0..=28.0),
                    )
                    .on_hover_text(tr("The theme's suggestion. The font size in Settings wins over it."))
                    .changed()
                {
                    changed = true;
                }
            });
            ui.small(tr("Which set of egui widget colours the chrome is built on."));

            ui.add_space(8.0);
            ui.strong(tr("Window buttons"));
            ui.horizontal(|ui| {
                ui.add_enabled_ui(editable, |ui| {
                    ui.label(tr("Style"));
                    for style in WindowButtonStyle::ALL {
                        let label = match style {
                            WindowButtonStyle::Stroke => "Stroked",
                            WindowButtonStyle::Aqua => "Aqua",
                            WindowButtonStyle::Luna => "Luna",
                        };
                        if ui
                            .selectable_label(theme.window_buttons.style == style, tr(label))
                            .clicked()
                        {
                            theme.window_buttons.style = style;
                            changed = true;
                        }
                    }
                    if ui
                        .checkbox(&mut theme.window_buttons.left, tr("On the left"))
                        .changed()
                    {
                        changed = true;
                    }
                });
            });
            ui.small(tr("Settings -> Window turns the buttons off altogether."));
            // What each colour falls back to when the theme does not set it,
            // which is what the swatch has to show rather than a black hole.
            let style = theme.window_buttons.style;
            let fallback =
                |slot: crate::config::theme::WindowButtonSlot| match style.default_colour(slot) {
                    Some(colour) => colour,
                    None => theme.ui_foreground,
                };
            let buttons = &mut theme.window_buttons;
            Grid::new("theme-buttons").num_columns(2).show(ui, |ui| {
                use crate::config::theme::WindowButtonSlot as Slot;
                if optional_colour_row(
                    ui,
                    "Close",
                    &mut buttons.close,
                    fallback(Slot::Close),
                    editable,
                    Some(&mut buttons.show_close),
                ) {
                    changed = true;
                }
                if optional_colour_row(
                    ui,
                    "Minimize",
                    &mut buttons.minimize,
                    fallback(Slot::Minimize),
                    editable,
                    Some(&mut buttons.show_minimize),
                ) {
                    changed = true;
                }
                if optional_colour_row(
                    ui,
                    "Maximize",
                    &mut buttons.maximize,
                    fallback(Slot::Maximize),
                    editable,
                    Some(&mut buttons.show_maximize),
                ) {
                    changed = true;
                }
                if optional_colour_row(
                    ui,
                    "Glyph",
                    &mut buttons.icon,
                    theme.ui_foreground,
                    editable,
                    None,
                ) {
                    changed = true;
                }
                if optional_colour_row(
                    ui,
                    "Close hover",
                    &mut buttons.hover_close,
                    theme.ui_foreground,
                    editable,
                    None,
                ) {
                    changed = true;
                }
            });

            ui.add_space(8.0);
            ui.strong(tr("ANSI"));
            ui.small(tr("0-7 normal, 8-15 bright: the sixteen colours IRIS can ask for by number."));
            Grid::new("theme-ansi").num_columns(8).show(ui, |ui| {
                for half in 0..2 {
                    for slot in 0..8 {
                        let index = half * 8 + slot;
                        ui.vertical(|ui| {
                            ui.small(format!("{index}"));
                            if ui
                                .add_enabled_ui(editable, |ui| {
                                    color_edit_button_srgba(
                                        ui,
                                        &mut theme.ansi[index],
                                        Alpha::Opaque,
                                    )
                                    .changed()
                                })
                                .inner
                            {
                                changed = true;
                            }
                        });
                    }
                    ui.end_row();
                }
            });

            ui.add_space(8.0);
            ui.strong(tr("ObjectScript syntax"));
            ui.small(tr("Named after the semantic token scopes of the InterSystems VS Code extension, so an editor colour customisation can be copied across field by field."));
            Grid::new("theme-syntax").num_columns(2).show(ui, |ui| {
                colour_rows!(
                    ui,
                    theme,
                    editable,
                    changed,
                    ("Label", syntax_label),
                    ("Command", syntax_command),
                    ("String", syntax_string),
                    ("Number", syntax_number),
                    ("Delimiter", syntax_delimiter),
                    ("Operator", syntax_operator),
                    ("Preprocessor", syntax_preprocessor),
                    ("Function", syntax_function),
                    ("Global", syntax_global),
                    ("System variable", syntax_system_variable),
                    ("Class", syntax_class),
                    ("Method", syntax_method),
                    ("Attribute", syntax_attribute),
                    ("Member", syntax_member),
                    ("Routine", syntax_routine),
                    ("Extrinsic", syntax_extrinsic),
                );
            });

            if let Some(path) = theme.path.as_ref() {
                ui.add_space(8.0);
                ui.small(tr1("Saved in {}", &path.display().to_string()));
            }
        });

    changed
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
/// The point of the pane: a hex value in a swatch says nothing about whether a
/// global is readable against the background. The lines below go through the
/// real scanner, so what is coloured here is exactly what would be coloured in
/// the session.
fn preview(ui: &mut Ui, theme: &Theme) {
    const SIZE: f32 = 12.0;

    ui.set_min_width(320.0);
    ui.set_max_width(340.0);
    ui.strong(tr("Preview"));

    // The chrome: what the tab strip and the window buttons look like, which no
    // swatch in the editor can show.
    egui::Frame::none()
        .fill(theme.ui_background)
        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if theme.window_buttons.left {
                    crate::ui::chrome::sample_buttons(ui, &theme.window_buttons);
                    ui.add_space(4.0);
                }
                ui.label(
                    egui::RichText::new("USER  100x30")
                        .color(theme.ui_foreground)
                        .size(SIZE),
                );
                if !theme.window_buttons.left {
                    crate::ui::chrome::sample_buttons(ui, &theme.window_buttons);
                }
            });
        });

    // The terminal itself.
    egui::Frame::none()
        .fill(theme.background)
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for line in SAMPLE {
                ui.label(coloured_line(line, theme, SIZE));
            }
            // A selected word and the cursor, the two colours nothing else in
            // the sample would show.
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontId::monospace(SIZE);
            job.append(
                "USER>",
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: theme.foreground,
                    ..Default::default()
                },
            );
            job.append(
                "selected",
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: theme.foreground,
                    background: theme.selection,
                    ..Default::default()
                },
            );
            job.append(
                "\u{2588}",
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: theme.cursor,
                    ..Default::default()
                },
            );
            ui.label(job);
        });

    // The sixteen ANSI slots, in the two rows they are numbered in.
    ui.add_space(4.0);
    ui.small(tr("ANSI"));
    for half in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for slot in 0..8 {
                let (rect, _) =
                    ui.allocate_exact_size(egui::Vec2::new(14.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 1.0, theme.ansi[half * 8 + slot]);
            }
        });
    }
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

    /// The editor follows the active theme until something else is picked, so
    /// opening the manager shows what is on screen.
    #[test]
    fn the_editor_starts_on_the_active_theme() {
        let themes = themes();
        let state = ThemeManagerState::default();
        assert_eq!(state.showing(&themes, "Tokyo").as_deref(), Some("Tokyo"));

        // A selection that no longer exists - the theme was deleted - falls
        // back rather than leaving the pane empty.
        let state = ThemeManagerState {
            selected: Some("Gone".into()),
            ..ThemeManagerState::default()
        };
        assert_eq!(state.showing(&themes, "Light").as_deref(), Some("Light"));
    }
}
