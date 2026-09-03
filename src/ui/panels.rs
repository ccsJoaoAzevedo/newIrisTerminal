//! Side panels and dialogs: macros, natives, settings, and export.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.

use egui::{Context, Ui};

use crate::config::profile::Remote;
use crate::config::servers::{Server, ServerList, Target};
use crate::config::{profile::LogMode, CursorStyle, Profile, Settings, Theme};
use crate::features::macros::{Macro, MacroGroup, Origin, Param};
use crate::features::natives::Native;
use crate::i18n::{tr, tr1, tr2};
use crate::term::Encoding;
use crate::ui::shortcut;

/// Colour for "this is set, but it will not do what you expect". Not from the
/// theme: it has to stay legible as a warning in every one of them.
pub const WARNING: egui::Color32 = egui::Color32::from_rgb(220, 120, 60);

/// Shown in the font picker for "whatever egui ships with".
const BUILT_IN_FONT: &str = "Built-in monospace";

/// Something the user asked for. `app.rs` decides whether and how to honour it.
#[derive(Clone, Debug)]
pub enum UiRequest {
    /// Send a macro, after confirmation if it asks for one.
    RunMacro(Macro),
    /// Send a native-utility invocation, with one value per parameter.
    RunNative(Native, Vec<String>),
    /// Send literal lines to the active session.
    SendLines(Vec<String>),
    ExportText(crate::features::export::Range),
    ExportHtml(crate::features::export::Range),
    CopyRange(crate::features::export::Range),
    SettingsChanged,
    /// Write the personal macro file back to disk.
    SavePersonalMacros,
    /// Show a folder in the platform's file manager.
    OpenFolder(std::path::PathBuf),
    /// Ask GitHub whether there is a newer version, and say either way.
    CheckForUpdates,
}

/// State the panels own between frames.
#[derive(Default)]
pub struct PanelState {
    pub show_macros: bool,
    pub show_settings: bool,
    pub show_export: bool,
    /// The theme manager, which keeps its own selection and rename draft.
    pub themes: crate::ui::theme_manager::ThemeManagerState,
    pub macro_filter: String,
    /// A macro waiting on parameter values and/or confirmation.
    pub pending: Option<PendingMacro>,
    /// One value per parameter of the selected native helper.
    pub native_values: Vec<String>,
    pub selected_native: Option<Native>,
    /// Which helper `native_values` belongs to, so folding one away and opening
    /// it again keeps what was typed while switching to a different one starts
    /// from that one's defaults.
    native_values_of: Option<Native>,
    /// Macro shown in the details block, by (group, index).
    pub selected: Option<(usize, usize)>,
    /// The personal macro currently open in the editor, by (group, index).
    pub editing: Option<(usize, usize)>,
    /// Draft being edited, kept separate so Cancel is a real cancel.
    pub draft: Option<Macro>,
    /// The editor is waiting for a key combination to be pressed, so that it
    /// can be read off the keyboard instead of typed out. Public because the
    /// app has to stop claiming shortcuts for itself while it is set, or Ctrl+T
    /// would open a tab rather than be recorded.
    pub capture_shortcut: bool,
    /// Whether a hidden body is currently shown in the editor. Deliberately not
    /// persisted and cleared every time the editor closes, so opening a macro
    /// never starts by putting its password on screen.
    reveal_body: bool,
    /// Group name for a macro about to be created.
    pub new_group: String,
    /// Installed monospace families, listed once. Enumerating system fonts is
    /// slow enough that doing it per frame would be felt while the Settings
    /// window is open.
    font_families: Option<Vec<String>>,
}

impl PanelState {
    /// Shows one IRIS helper's fields, or `None` to fold the open one away.
    ///
    /// The fields are only reset when a *different* helper is opened; the
    /// compile flag starts at its default rather than blank, and re-opening the
    /// helper you just closed gives you back what you had typed.
    fn open_native(&mut self, native: Option<Native>) {
        self.selected_native = native;
        if let Some(native) = native {
            if self.native_values_of != Some(native) {
                self.native_values = native.default_values();
                self.native_values_of = Some(native);
            }
        }
    }

    /// Opens the editor on one macro, on a copy of it.
    fn open_editor(&mut self, at: (usize, usize), draft: Macro) {
        self.editing = Some(at);
        self.draft = Some(draft);
        self.reveal_body = false;
        self.capture_shortcut = false;
    }
}

#[derive(Clone, Debug)]
pub struct PendingMacro {
    pub source: Macro,
    pub values: Vec<(String, String)>,
    /// Set once parameters are filled and only the yes/no remains.
    pub confirming: bool,
}

impl PendingMacro {
    pub fn new(source: Macro) -> Self {
        let values = source.default_values();
        let confirming = values.is_empty() && source.confirm;
        PendingMacro {
            source,
            values,
            confirming,
        }
    }

    pub fn preview(&self) -> Vec<String> {
        self.source.expand(&self.values)
    }
}

/// The macro browser: pick a macro, see what it will do, then run it.
///
/// Selection rather than run-on-click. A list where clicking a name fires the
/// command underneath it has no room to show what that command is, and half
/// these macros write to a shared database — so the click selects, the details
/// below say what would happen, and Run is a separate, deliberate press.
/// Clicking the open macro again folds those details away.
///
/// Takes the groups mutably because personal macros are edited in place here;
/// organisation macros are shown with a badge and no edit affordance, since
/// their file is shared and never written from the app.
pub fn macros_panel(
    ui: &mut Ui,
    groups: &mut Vec<MacroGroup>,
    state: &mut PanelState,
) -> Option<UiRequest> {
    let mut request = None;

    // The panel says what it is and offers the way out. Clicking Macros in the
    // menu bar again still closes it - this is the same gesture put where a
    // panel is normally closed from, since nothing on screen said that the
    // toggle in the bar was the only way back.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tr("Macros")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("x")
                .on_hover_text(tr("Close this panel"))
                .clicked()
            {
                state.show_macros = false;
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(tr("Filter"));
        ui.text_edit_singleline(&mut state.macro_filter);
    });
    ui.separator();

    if groups.is_empty() {
        ui.label(tr("No macros defined."));
        ui.small(tr(
            "Add them below, or configure the organization file in Settings.",
        ));
    }

    // An index into a list that has since been reloaded, or had a macro
    // deleted, would point at the wrong macro; a stale selection is dropped
    // rather than followed.
    if let Some((gi, mi)) = state.selected {
        if groups.get(gi).and_then(|g| g.macros.get(mi)).is_none() {
            state.selected = None;
        }
    }

    let filter = state.macro_filter.to_lowercase();
    // `Some(None)` is "fold the details away", which is what clicking the open
    // macro again means; `None` is "nothing was clicked".
    let mut to_select: Option<Option<(usize, usize)>> = None;
    let mut to_run: Option<Macro> = None;

    // No scroll area of its own: the whole panel scrolls, so a long macro list
    // is not squeezed into half the height with the details below it fighting
    // for the rest.
    for (gi, group) in groups.iter().enumerate() {
        let matching: Vec<(usize, &Macro)> = group
            .macros
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                filter.is_empty()
                    || m.name.to_lowercase().contains(&filter)
                    || m.description.to_lowercase().contains(&filter)
            })
            .collect();
        if matching.is_empty() {
            continue;
        }

        let title = if group.name.is_empty() {
            "Macros"
        } else {
            &group.name
        };
        egui::CollapsingHeader::new(title)
            .id_source(("macro-group", gi))
            .default_open(true)
            .show(ui, |ui| {
                for (mi, m) in matching {
                    ui.horizontal(|ui| {
                        // Run sits on the row with the name, so firing
                        // a macro is one press from the list. It still
                        // goes through the same request path, so a
                        // macro that asks for parameters or a
                        // confirmation asks for them here too.
                        if ui
                            .small_button(tr("Run"))
                            .on_hover_text(if m.confirm {
                                tr("Runs after confirming.")
                            } else {
                                tr("Sends this macro to the active session.")
                            })
                            .clicked()
                        {
                            to_run = Some(m.clone());
                        }
                        let label = if m.confirm {
                            format!("{}  ({})", m.name, tr("confirms"))
                        } else {
                            m.name.clone()
                        };
                        let selected = state.selected == Some((gi, mi));
                        let button = ui.selectable_label(selected, label);
                        let button = if m.description.is_empty() {
                            button
                        } else {
                            button.on_hover_text(&m.description)
                        };
                        if button.clicked() {
                            // Clicking the open one again folds its
                            // details away, the same gesture the IRIS
                            // utilities below use.
                            to_select = Some((!selected).then_some((gi, mi)));
                        }

                        // Provenance at a glance: a shared macro
                        // behaving oddly is someone else's file, not
                        // something the user can have broken locally.
                        if m.origin == Origin::Organization {
                            ui.weak(tr("org"))
                                .on_hover_text(tr("Provided by the organization; read-only here."));
                        }

                        // The shortcut belongs next to the name. A
                        // binding nobody can see is a binding nobody
                        // uses.
                        if let Some(key) = &m.key {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.weak(key);
                                },
                            );
                        }
                    });
                }
            });
    }

    if let Some(at) = to_select {
        state.selected = at;
    }
    if let Some(m) = to_run {
        request = Some(UiRequest::RunMacro(m));
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(tr("New in group"));
        ui.add(egui::TextEdit::singleline(&mut state.new_group).desired_width(90.0));
        let named = !state.new_group.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new("Add")).clicked() {
            let name = state.new_group.trim().to_string();
            let gi = match groups.iter().position(|g| g.name == name) {
                Some(i) => i,
                None => {
                    groups.push(MacroGroup {
                        name,
                        origin: Origin::Personal,
                        macros: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            groups[gi].macros.push(Macro {
                origin: Origin::Personal,
                name: "New macro".into(),
                ..Macro::default()
            });
            let mi = groups[gi].macros.len() - 1;
            state.selected = Some((gi, mi));
            state.open_editor((gi, mi), groups[gi].macros[mi].clone());
        }
    });

    if let Some(r) = macro_details(ui, groups, state) {
        request = Some(r);
    }
    request
}

/// What the selected macro is, what it would send, and the actions on it.
fn macro_details(
    ui: &mut Ui,
    groups: &mut Vec<MacroGroup>,
    state: &mut PanelState,
) -> Option<UiRequest> {
    let (gi, mi) = state.selected?;
    let m = groups.get(gi).and_then(|g| g.macros.get(mi))?.clone();

    let mut request = None;
    let mut delete = false;

    ui.separator();
    ui.heading(&m.name);
    if !m.description.is_empty() {
        ui.label(&m.description);
    }

    // No Run here: it lives next to the name in the list above, where it is
    // reachable without selecting the macro first.
    ui.horizontal(|ui| match m.origin {
        Origin::Personal => {
            if ui.button(tr("Edit")).clicked() {
                state.open_editor((gi, mi), m.clone());
            }
            if ui
                .button(tr("Delete"))
                .on_hover_text(tr("Removes it from your personal macro file."))
                .clicked()
            {
                delete = true;
            }
        }
        Origin::Organization => {
            ui.weak(tr("Provided by the organization; read-only here."));
        }
    });

    if let Some(key) = &m.key {
        ui.horizontal(|ui| {
            ui.label(tr("Shortcut"));
            ui.weak(key);
            if shortcut::parse(key).is_none() {
                ui.colored_label(WARNING, tr("not a shortcut this app understands"));
            }
        });
    }

    if !m.params.is_empty() {
        ui.label(tr("Parameters"));
        for p in &m.params {
            let prompt = if p.prompt.is_empty() {
                &p.name
            } else {
                &p.prompt
            };
            ui.small(format!("{}  -  {prompt}", p.name));
        }
    }

    if m.hide_command {
        // The whole point of the flag: this body holds a credential, so the
        // panel does not put it on screen just because the macro is selected.
        ui.weak(hidden_body_note(&m));
    } else {
        for line in &m.body {
            ui.code(line);
        }
    }

    if delete {
        // Guarded by construction - Delete only appears on personal macros -
        // but checked anyway so a future refactor cannot destroy shared data.
        if groups[gi].macros[mi].origin.is_editable() {
            groups[gi].macros.remove(mi);
            if groups[gi].macros.is_empty() {
                groups.remove(gi);
            }
            state.selected = None;
            state.editing = None;
            state.draft = None;
            request = Some(UiRequest::SavePersonalMacros);
        }
    }

    request
}

/// How many body lines a hidden macro has, without saying what they are.
fn hidden_body_note(m: &Macro) -> String {
    match m.body.len() {
        1 => tr("1 command hidden").to_string(),
        n => tr1("{} commands hidden", &n.to_string()),
    }
}

/// Editor for one personal macro, in a window of its own.
///
/// A window rather than another section stacked under the list: the sidebar is
/// for choosing and reading, and an editor wedged into the bottom of it left
/// neither enough room.
pub fn macro_editor_dialog(
    ctx: &Context,
    groups: &mut [MacroGroup],
    state: &mut PanelState,
) -> Option<UiRequest> {
    let (gi, mi) = state.editing?;
    if groups.get(gi).and_then(|g| g.macros.get(mi)).is_none() {
        state.editing = None;
        state.draft = None;
        return None;
    }

    let mut request = None;
    let mut close = false;
    let mut open = true;
    let mut reveal = state.reveal_body;
    let mut capture = state.capture_shortcut;

    egui::Window::new(tr("Edit macro"))
        .id(egui::Id::new("nit-macro-editor"))
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let Some(draft) = state.draft.as_mut() else {
                close = true;
                return;
            };

            ui.horizontal(|ui| {
                ui.label(tr("Name"));
                ui.text_edit_singleline(&mut draft.name);
            });
            ui.horizontal(|ui| {
                ui.label(tr("Description"));
                ui.text_edit_singleline(&mut draft.description);
            });

            ui.horizontal(|ui| {
                ui.label(tr("Shortcut"));
                let mut key = draft.key.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut key)
                            .hint_text("Ctrl+Shift+G")
                            .desired_width(140.0),
                    )
                    .changed()
                {
                    let key = key.trim().to_string();
                    draft.key = (!key.is_empty()).then_some(key);
                }
                // Typing the name of a chord is fiddly and easy to get subtly
                // wrong - `Num7` against `7`, `Option` against `Alt` - so the
                // other way in is to press it. What lands in the field is what
                // the parser produced, which is the value that will fire.
                let label = if capture {
                    tr("Press the keys...")
                } else {
                    tr("Detect")
                };
                if ui
                    .selectable_label(capture, label)
                    .on_hover_text(tr(
                        "Press the combination and it is filled in here. Esc cancels, Backspace clears it.",
                    ))
                    .clicked()
                {
                    capture = !capture;
                }
            });
            if capture {
                match captured_shortcut(ui) {
                    Capture::Waiting => {}
                    Capture::Cancelled => capture = false,
                    Capture::Cleared => {
                        draft.key = None;
                        capture = false;
                    }
                    Capture::Chord(text) => {
                        draft.key = Some(text);
                        capture = false;
                    }
                }
                ui.small(tr("A modifier is required: Ctrl, Alt, or both, with or without Shift."));
            }
            // Reported rather than rejected: this field is also edited by hand
            // in the shared XML, and a value we do not understand has to
            // survive a round trip through here instead of being erased.
            if let Some(key) = draft.key.as_deref() {
                match shortcut::parse(key) {
                    None => {
                        ui.colored_label(
                            WARNING,
                            tr("Not understood, so it will not fire. Needs a modifier, like Ctrl+Shift+G."),
                        );
                    }
                    Some((modifiers, parsed)) => {
                        if let Some(used_for) = shortcut::is_reserved(modifiers, parsed) {
                            ui.colored_label(
                                WARNING,
                                tr1("The app already uses this for {}; add Shift.", used_for),
                            );
                        }
                    }
                }
            }

            ui.checkbox(
                &mut draft.confirm,
                tr("Confirm before sending (use for anything that writes)"),
            );
            if ui
                .checkbox(&mut draft.hide_command, tr("Hide command"))
                .on_hover_text(
                    tr("For a body that carries a password. Keeps it out of the macro panel; IRIS still echoes what it is sent."),
                )
                .changed()
            {
                // Ticking the box hides the body again immediately, so the
                // secret is not left on screen by the act of protecting it.
                reveal = false;
            }

            ui.label(tr("Body - one command per line, {{param}} is substituted"));
            if draft.hide_command && !reveal {
                ui.horizontal(|ui| {
                    ui.weak(hidden_body_note(draft));
                    if ui.button(tr("Reveal")).clicked() {
                        reveal = true;
                    }
                });
            } else {
                let mut body = draft.body.join("\n");
                if ui
                    .add(egui::TextEdit::multiline(&mut body).desired_rows(4))
                    .changed()
                {
                    draft.body = body
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect();
                }
            }

            ui.label(tr("Parameters"));
            let mut drop_param = None;
            for (pi, param) in draft.params.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut param.name).desired_width(60.0))
                        .on_hover_text(tr("Name used as {{name}} in the body"));
                    ui.add(egui::TextEdit::singleline(&mut param.prompt).desired_width(100.0))
                        .on_hover_text(tr("Prompt shown when running"));
                    ui.add(egui::TextEdit::singleline(&mut param.default).desired_width(70.0))
                        .on_hover_text(tr("Default value"));
                    if ui.small_button("x").clicked() {
                        drop_param = Some(pi);
                    }
                });
            }
            if let Some(pi) = drop_param {
                draft.params.remove(pi);
            }
            if ui.small_button(tr("Add parameter")).clicked() {
                draft.params.push(Param::default());
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("Save")).clicked() {
                    // Origin is never taken from the draft: an edited macro
                    // stays personal, so nothing can promote itself into the
                    // shared file.
                    let mut saved = draft.clone();
                    saved.origin = Origin::Personal;
                    groups[gi].macros[mi] = saved;
                    close = true;
                    request = Some(UiRequest::SavePersonalMacros);
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            });
        });

    state.reveal_body = reveal;
    state.capture_shortcut = capture;

    if close || !open {
        state.editing = None;
        state.draft = None;
        state.reveal_body = false;
        state.capture_shortcut = false;
    }
    request
}

/// What a frame of key presses meant while the editor was listening for a
/// shortcut.
enum Capture {
    /// Nothing usable yet, so keep listening.
    Waiting,
    /// Escape: leave the binding as it was.
    Cancelled,
    /// Backspace or Delete: no shortcut at all.
    Cleared,
    /// A chord, in the parser's own spelling.
    Chord(String),
}

/// Takes the pressed chord out of this frame's events.
///
/// Every key press is consumed while listening, and so is the text they would
/// have produced: a key pressed here is the shortcut being named, not typing,
/// and leaving it in the stream would put a letter in the name field or fire
/// the very shortcut being recorded. A chord without Ctrl or Alt is ignored
/// rather than accepted - a bare letter, or Shift plus one, would fire while
/// the user was typing at the prompt.
fn captured_shortcut(ui: &Ui) -> Capture {
    ui.input_mut(|input| {
        let mut result = Capture::Waiting;
        input.events.retain(|event| match event {
            egui::Event::Text(_) => false,
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => {
                match key {
                    egui::Key::Escape => result = Capture::Cancelled,
                    egui::Key::Backspace | egui::Key::Delete => result = Capture::Cleared,
                    key if modifiers.ctrl || modifiers.alt || modifiers.command => {
                        result = Capture::Chord(shortcut::format(*modifiers, *key));
                    }
                    _ => {}
                }
                false
            }
            _ => true,
        });
        result
    })
}

/// Parameter-fill and confirmation dialog for a pending macro.
///
/// The preview is not decoration: it is the last chance to notice that a
/// substituted value points at the wrong global before the text reaches a
/// shared database.
pub fn pending_macro_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    let pending = state.pending.as_mut()?;

    let mut request = None;
    let mut close = false;
    let mut open = true;

    let title = if pending.confirming {
        tr1("Confirm: {}", &pending.source.name)
    } else {
        pending.source.name.clone()
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if !pending.source.description.is_empty() {
                ui.label(&pending.source.description);
                ui.separator();
            }

            for (index, param) in pending.source.params.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&param.prompt);
                    if let Some((_, value)) = pending.values.get_mut(index) {
                        ui.text_edit_singleline(value);
                    }
                });
            }

            ui.separator();
            ui.label(tr("Will send:"));
            let preview = pending.source.expand(&pending.values);
            for line in &preview {
                ui.code(line);
            }

            if pending.source.confirm {
                ui.separator();
                ui.colored_label(
                    WARNING,
                    tr("This macro is marked as modifying data. RDB* databases are shared with the team."),
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                let send_label = if pending.source.confirm {
                    tr("Yes, send it")
                } else {
                    tr("Send")
                };
                if ui.button(send_label).clicked() {
                    request = Some(UiRequest::SendLines(preview.clone()));
                    close = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            });
        });

    if close || !open {
        state.pending = None;
    }
    request
}

/// The native-utility helpers.
pub fn natives_panel(ui: &mut Ui, state: &mut PanelState) -> Option<UiRequest> {
    let mut request = None;

    ui.label(tr("IRIS utilities"));
    ui.separator();

    for native in Native::ALL {
        let selected = state.selected_native == Some(native);
        if ui.selectable_label(selected, tr(native.label())).clicked() {
            // Clicking the open one again folds it away rather than resetting
            // its fields, which is what the same click used to do - and losing
            // a typed package name to a stray click is a poor trade for a
            // gesture that reads like "close this".
            state.open_native(if selected { None } else { Some(native) });
        }
    }

    if let Some(native) = state.selected_native {
        ui.separator();
        // A selection made before the last reload could be holding fewer
        // values than the helper now asks for.
        state
            .native_values
            .resize(native.params().len(), String::new());
        for (param, value) in native.params().iter().zip(state.native_values.iter_mut()) {
            ui.label(tr(param.label));
            ui.text_edit_singleline(value);
        }

        let invocation = native.build(&state.native_values);
        ui.add_space(8.0);
        ui.label(tr("Will send:"));
        for line in &invocation.lines {
            ui.code(line);
        }
        ui.add_space(8.0);
        if ui.button(tr("Run")).clicked() {
            request = Some(UiRequest::RunNative(native, state.native_values.clone()));
        }
    }

    request
}

/// Export / copy actions.
pub fn export_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    use crate::features::export::Range;

    if !state.show_export {
        return None;
    }
    let mut request = None;
    let mut open = true;

    egui::Window::new(tr("Export output"))
        .id(egui::Id::new("nit-export"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(tr("Save to a file"));
            ui.horizontal(|ui| {
                if ui.button(tr("Screen as text")).clicked() {
                    request = Some(UiRequest::ExportText(Range::Screen));
                }
                if ui.button(tr("Everything as text")).clicked() {
                    request = Some(UiRequest::ExportText(Range::All));
                }
            });
            ui.horizontal(|ui| {
                if ui.button(tr("Screen as HTML")).clicked() {
                    request = Some(UiRequest::ExportHtml(Range::Screen));
                }
                if ui.button(tr("Everything as HTML")).clicked() {
                    request = Some(UiRequest::ExportHtml(Range::All));
                }
            });

            ui.separator();
            ui.label(tr("Copy to the clipboard"));
            ui.horizontal(|ui| {
                if ui.button(tr("Screen")).clicked() {
                    request = Some(UiRequest::CopyRange(Range::Screen));
                }
                if ui.button(tr("Everything")).clicked() {
                    request = Some(UiRequest::CopyRange(Range::All));
                }
            });
        });

    if !open {
        state.show_export = false;
    }
    if request.is_some() {
        state.show_export = false;
    }
    request
}

/// Offers the newer version that was found. Returns true when the user asked
/// for it to be applied.
///
/// Two steps, because they are two different waits: the download runs on a
/// thread and can take a while over a proxy, and only once it is on disk is
/// there anything to restart into.
pub fn update_dialog(ctx: &Context, state: &mut crate::app::UpdateState) -> bool {
    let Some(release) = state.available.clone() else {
        return false;
    };
    if !state.asked {
        return false;
    }
    let mut apply = false;
    let mut open = true;

    egui::Window::new(tr("Update available"))
        .id(egui::Id::new("nit-update"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(tr2(
                "Version {} is available. This one is {}.",
                &release.version,
                crate::features::update::CURRENT,
            ));
            if !release.notes.is_empty() {
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_source("update-notes")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.small(&release.notes);
                    });
            }
            if let Some(error) = state.error.as_ref() {
                ui.add_space(4.0);
                ui.colored_label(WARNING, error);
            }
            ui.add_space(6.0);
            ui.separator();

            ui.horizontal(|ui| {
                if state.downloading {
                    ui.spinner();
                    ui.label(tr("Downloading..."));
                    return;
                }
                if state.staged.is_some() {
                    if ui
                        .button(tr("Restart and update"))
                        .on_hover_text(tr(
                            "Puts the new version in place and starts it. A session still connected is asked about first.",
                        ))
                        .clicked()
                    {
                        apply = true;
                    }
                } else if ui.button(tr("Download")).clicked() {
                    state.start_download();
                }
                if ui.button(tr("Later")).clicked() {
                    // Dismissed for this run. The next start asks again, which
                    // is the whole of the nagging this does.
                    state.asked = false;
                }
            });
        });

    if !open {
        state.asked = false;
    }
    apply
}

/// A heading with room around it, so the sections of the settings window read
/// as sections rather than as one list with bold lines in it.
fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.separator();
    ui.heading(title);
    ui.add_space(2.0);
}

/// Settings: theme, font, scrollback, logging, and profiles.
#[allow(clippy::too_many_arguments)]
pub fn settings_dialog(
    ctx: &Context,
    settings: &mut Settings,
    themes: &[Theme],
    state: &mut PanelState,
    instances: &[String],
    servers: &ServerList,
    placement: &mut crate::ui::detach::Placement,
    buttons: &crate::config::theme::WindowButtons,
) -> Option<UiRequest> {
    if !state.show_settings {
        return None;
    }
    let mut changed = false;
    let mut open = true;
    // Kept apart from `changed`: opening a folder is an action, not an edit,
    // and it must not make the app rewrite settings.toml.
    let mut action: Option<UiRequest> = None;
    // Read before the closure borrows `settings`: the window's own frame is
    // drawn around the very setting that decides whether it has one.
    let native_decorations = settings.native_decorations;

    crate::ui::detach::shell(
        ctx,
        "nit-settings",
        tr("Settings"),
        &mut open,
        [560.0, 680.0],
        buttons,
        native_decorations,
        // A window of its own, so it reopens where and how it was left for
        // whichever of the two switches below is on.
        Some(placement),
        |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Roomier than egui's default. A settings window is read down
                // the page one item at a time, and at the default 3pt the
                // checkboxes and their hints ran together into a wall.
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.heading(tr("Appearance"));
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Language"));
                    egui::ComboBox::from_id_source("language-picker")
                        .selected_text(settings.language.label())
                        .show_ui(ui, |ui| {
                            for lang in crate::i18n::Lang::ALL {
                                if ui
                                    .selectable_label(settings.language == lang, lang.label())
                                    .clicked()
                                {
                                    settings.language = lang;
                                    // Applied at once rather than at the next
                                    // start: the rest of this window is what
                                    // anyone changing the language wants to
                                    // read in it.
                                    crate::i18n::set_language(lang);
                                    changed = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Theme"));
                    egui::ComboBox::from_id_source("theme-picker")
                        .selected_text(settings.theme.clone())
                        .show_ui(ui, |ui| {
                            for theme in themes {
                                if ui
                                    .selectable_label(theme.name == settings.theme, &theme.name)
                                    .clicked()
                                {
                                    settings.theme = theme.name.clone();
                                    changed = true;
                                }
                            }
                        });
                    // Editing a theme by hand is still possible, but it is no
                    // longer the only way: the manager is what the button next
                    // to the picker offers first.
                    if ui
                        .button(tr("Manage themes..."))
                        .on_hover_text(tr("Duplicate a built-in theme and change any of its colours, including the ObjectScript ones."))
                        .clicked()
                    {
                        state.themes.open = true;
                    }
                    if ui
                        .button(tr("Open folder"))
                        .on_hover_text(crate::config::themes_dir().display().to_string())
                        .clicked()
                    {
                        action = Some(UiRequest::OpenFolder(crate::config::themes_dir()));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Font"));
                    let families = state
                        .font_families
                        .get_or_insert_with(crate::ui::fonts::monospace_families);
                    let selected = if settings.font_family.is_empty() {
                        tr(BUILT_IN_FONT)
                    } else {
                        settings.font_family.as_str()
                    };
                    egui::ComboBox::from_id_source("font-picker")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    settings.font_family.is_empty(),
                                    tr(BUILT_IN_FONT),
                                )
                                .clicked()
                            {
                                settings.font_family.clear();
                                changed = true;
                            }
                            for family in families.iter() {
                                if ui
                                    .selectable_label(&settings.font_family == family, family)
                                    .clicked()
                                {
                                    settings.font_family = family.clone();
                                    changed = true;
                                }
                            }
                        });
                });
                ui.small(tr("Monospace families only: the terminal is a character grid, so a proportional font would not line up."));

                ui.horizontal(|ui| {
                    ui.label(tr("Font size"));
                    if ui
                        .add(egui::Slider::new(&mut settings.font_size, 8.0..=28.0))
                        .changed()
                    {
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(tr("Cursor"));
                    for style in CursorStyle::ALL {
                        if ui
                            .selectable_label(settings.cursor_style == style, tr(style.label()))
                            .clicked()
                        {
                            settings.cursor_style = style;
                            changed = true;
                        }
                    }
                    if ui.checkbox(&mut settings.cursor_blink, tr("Blink")).changed() {
                        changed = true;
                    }
                });

                if ui
                    .checkbox(
                        &mut settings.terminal_syntax_highlight,
                        tr("Syntax highlighting"),
                    )
                    .on_hover_text(
                        tr("Colours globals, strings, numbers, commands, macros and class references. A guess about the text on screen; a colour IRIS sets itself always wins."),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut settings.show_scrollbars, tr("Show scrollbars"))
                    .on_hover_text(
                        tr("Solid scrollbars instead of the thin ones that only appear on hover."),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small(
                    tr("The built-in themes are read-only; duplicating one in the theme manager gives you a copy to edit. A theme file dropped into the themes folder by hand is picked up at the next start."),
                );

                section(ui, tr("Session"));
                ui.horizontal(|ui| {
                    ui.label(tr("Hide status messages after"));
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.status_timeout_secs)
                                .range(0..=600)
                                .suffix(" s"),
                        )
                        .on_hover_text(tr(
                            "Seconds before a message in the footer goes away on its own. 0 leaves it until it is dismissed.",
                        ))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Scrollback lines"));
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.scrollback_limit).range(0..=200_000),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
                if ui
                    .checkbox(&mut settings.wrap_lines, tr("Wrap long lines"))
                    .on_hover_text(
                        tr("On: a long line continues on the next row, breaking at the window edge. Off: it runs off to the right, reached by scrolling sideways or widening the window."),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small(
                    tr("Either way the whole line is kept: the terminal is reported wider than the window, because IRIS cuts a line at the terminal width instead of wrapping it."),
                );

                if ui
                    .checkbox(&mut settings.copy_on_select, tr("Copy on select"))
                    .on_hover_text(
                        tr("Put a selection on the clipboard as soon as the mouse is released, without waiting for Ctrl+C."),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.recall_mid_line,
                        tr("Up and Down recall from anywhere on the line"),
                    )
                    .on_hover_text(tr(
                        "On: Up replaces the line with an earlier command wherever the cursor is, the way the native IRIS terminal does. Off: only at the end of the line, so a cursor left in the middle means the line is being edited and the arrows leave it alone.",
                    ))
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.save_command_history,
                        tr("Remember commands from earlier sessions"),
                    )
                    .on_hover_text(
                        tr("Keeps the commands typed at an IRIS prompt in history.txt, so Up reaches back past the sessions open now. Off keeps recall working inside each session and writes nothing to disk. Either way a tab offers back its own commands first and the inherited ones after them, and lines sent by a macro or an IRIS helper are never offered back at all."),
                    )
                    .changed()
                {
                    changed = true;
                }

                if ui
                    .checkbox(
                        &mut settings.show_namespace_in_tab,
                        tr("Show the namespace in the tab name"),
                    )
                    .on_hover_text(tr(
                        "Adds the namespace the session is in to the tab's name - CONSISTEM | RDB76-TR. Read off the prompt, so it follows a ZN as it happens; a tab renamed by hand keeps the name it was given.",
                    ))
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut settings.show_pid, tr("Show the process id"))
                    .on_hover_text(tr(
                        "Puts the session's process id next to the instance name and the window size in the menu bar. A local session only: a remote one runs its process on the far side.",
                    ))
                    .changed()
                {
                    changed = true;
                }

                if ui
                    .checkbox(
                        &mut settings.open_on_start,
                        tr("Open the default profile at startup"),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.confirm_close_with_live_session,
                        tr("Ask before closing with a session still connected"),
                    )
                    .changed()
                {
                    changed = true;
                }

                section(ui, tr("Window"));
                if ui
                    .checkbox(
                        &mut settings.native_decorations,
                        tr("Use the system title bar"),
                    )
                    .on_hover_text(
                        tr("Off by default: the app draws its own, which frees the row the system bar would take."),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.show_window_buttons,
                        tr("Show the window buttons"),
                    )
                    .on_hover_text(
                        tr("Minimize, maximize and close in the app's own title bar. Off leaves the row to the tabs: the window still drags, double-click still maximizes, and Ctrl+W still closes. The active theme decides how the buttons look and which end they sit at."),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Terminal size"));
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.default_cols)
                                .range(20..=500)
                                .prefix(format!("{} ", tr("Columns"))),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    for cols in crate::config::COMMON_COLS {
                        if ui
                            .selectable_label(settings.default_cols == cols, cols.to_string())
                            .clicked()
                        {
                            settings.default_cols = cols;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("     ");
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.default_rows)
                                .range(5..=200)
                                .prefix(format!("{} ", tr("Rows"))),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    for rows in crate::config::COMMON_ROWS {
                        if ui
                            .selectable_label(settings.default_rows == rows, rows.to_string())
                            .clicked()
                        {
                            settings.default_rows = rows;
                            changed = true;
                        }
                    }
                });
                ui.small(tr(
                    "The size a window opens at when it is not reopening at the last one. In characters, so it holds the same amount of output at any font size.",
                ));

                if ui
                    .checkbox(&mut settings.save_terminal_size, tr("Save terminal size"))
                    .on_hover_text(
                        tr("Reopens the window at the size it was last closed at. Off opens it at 100x30 characters, whatever the font size."),
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut settings.save_window_position, tr("Save window position"))
                    .on_hover_text(
                        tr("Reopens the window where it was last closed. Off centres it on the screen."),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small(tr(
                    "Takes effect the next time the app starts, and covers this window as well as the main one.",
                ));

                section(ui, tr("Updates"));
                if ui
                    .checkbox(
                        &mut settings.check_for_updates,
                        tr("Check for a new version at startup"),
                    )
                    .on_hover_text(tr(
                        "One request to GitHub through the machine's own proxy. Nothing is downloaded or replaced without being asked.",
                    ))
                    .changed()
                {
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label(tr1(
                        "This build is version {}.",
                        crate::features::update::CURRENT,
                    ));
                    if ui.button(tr("Check now")).clicked() {
                        action = Some(UiRequest::CheckForUpdates);
                    }
                });
                if let Some(proxy) = crate::features::update::system_proxy() {
                    ui.small(tr1("Going through the system proxy at {}.", &proxy));
                } else {
                    ui.small(tr("No system proxy configured; connecting directly."));
                }

                section(ui, tr("Macros"));
                ui.label(tr("Organization macro file (shared, read-only)"));
                let mut org = settings.org_macros_path.display().to_string();
                if ui.text_edit_singleline(&mut org).changed() {
                    settings.org_macros_path = std::path::PathBuf::from(org.trim());
                    changed = true;
                }
                ui.small(
                    tr("A UNC share, mapped drive, or local copy. Leave empty for none.                      Your personal macros are edited in the Macros panel."),
                );
                if let Some(path) = settings.org_macros() {
                    if path.exists() {
                        ui.small(tr("Found."));
                    } else {
                        ui.colored_label(
                            WARNING,
                            tr("Not reachable right now - personal macros will still load."),
                        );
                    }
                }

                section(ui, tr("Logging"));
                ui.horizontal(|ui| {
                    ui.label(tr("Default mode"));
                    for (mode, label) in [
                        (LogMode::Off, "Off"),
                        (LogMode::Clean, "Clean text"),
                        (LogMode::Raw, "Raw bytes"),
                    ] {
                        if ui
                            .selectable_label(settings.default_log_mode == mode, tr(label))
                            .clicked()
                        {
                            settings.default_log_mode = mode;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Keep logs for (days, 0 = forever)"));
                    if ui
                        .add(egui::DragValue::new(&mut settings.log_retention_days).range(0..=3650))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.small(tr1("Logs: {}", &settings.log_dir.display().to_string()));

                section(ui, tr("Profiles"));
                if profiles_editor(ui, settings, instances, servers) {
                    changed = true;
                }
            });
        },
    );

    if !open {
        state.show_settings = false;
    }
    action.or_else(|| changed.then_some(UiRequest::SettingsChanged))
}

/// Returns true when anything changed.
fn profiles_editor(
    ui: &mut Ui,
    settings: &mut Settings,
    instances: &[String],
    servers: &ServerList,
) -> bool {
    let mut changed = false;
    let mut remove = None;

    for (index, profile) in settings.profiles.iter_mut().enumerate() {
        egui::CollapsingHeader::new(if profile.name.is_empty() {
            tr1("Profile {}", &(index + 1).to_string())
        } else {
            profile.name.clone()
        })
        .id_source(("profile", index))
        .show(ui, |ui| {
            changed |= labelled_edit(ui, "Name", &mut profile.name);

            ui.horizontal(|ui| {
                ui.label(tr("Instance"));
                if servers.is_empty() && instances.is_empty() {
                    // Nothing was discovered - no launcher, or a machine this
                    // app cannot read the list on - so the name is typed.
                    changed |= ui.text_edit_singleline(&mut profile.instance).changed();
                } else {
                    egui::ComboBox::from_id_source(("instance", index))
                        .selected_text(instance_label(profile))
                        .show_ui(ui, |ui| {
                            changed |= instance_menu(ui, profile, instances, servers);
                        });
                }
            });
            // Where the session actually goes, since a server entry can be a
            // Telnet login rather than an instance on this machine.
            if let Some(remote) = profile.remote.as_ref() {
                ui.small(tr1(
                    "Telnet login to {}",
                    &format!("{}:{}", remote.address, remote.port),
                ));
            }

            // Only a Telnet session has a charset to choose. A local one runs
            // inside a pseudo-console, which hands the terminal UTF-8 whatever
            // codepage it is on, so there is nothing here that could change it -
            // offering the choice only invited a setting that costs a column
            // per accent. See `Profile::wire_encoding`.
            if profile.remote.is_some() {
                ui.horizontal(|ui| {
                    ui.label(tr("Encoding"));
                    egui::ComboBox::from_id_source(("encoding", index))
                        .selected_text(tr(profile.encoding.label()))
                        .show_ui(ui, |ui| {
                            for enc in Encoding::ALL {
                                if ui
                                    .selectable_label(profile.encoding == enc, tr(enc.label()))
                                    .clicked()
                                {
                                    profile.encoding = enc;
                                    changed = true;
                                }
                            }
                        });
                });
                ui.small(tr(
                    "Leave as UTF-8 unless accented characters come out wrong.",
                ));
            } else {
                ui.small(tr("A local session speaks UTF-8."));
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tr("Logging"));
                for (mode, label) in [
                    (LogMode::Off, "Use default"),
                    (LogMode::Clean, "Clean"),
                    (LogMode::Raw, "Raw"),
                ] {
                    if ui
                        .selectable_label(profile.logging == mode, tr(label))
                        .clicked()
                    {
                        profile.logging = mode;
                        changed = true;
                    }
                }
            });

            if ui.button(tr("Delete this profile")).clicked() {
                remove = Some(index);
            }
        });
    }

    if let Some(index) = remove {
        // Forget the stored secret too, or it outlives the profile that
        // explained what it was for.
        settings.profiles[index].clear_password();
        settings.profiles.remove(index);
        changed = true;
    }

    ui.horizontal(|ui| {
        if ui.button(tr("Add profile")).clicked() {
            settings.profiles.push(Profile {
                name: format!("Profile {}", settings.profiles.len() + 1),
                instance: instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });
            changed = true;
        }
        if !settings.profiles.is_empty() {
            ui.label(tr("Default"));
            egui::ComboBox::from_id_source("default-profile")
                .selected_text(settings.default_profile.clone())
                .show_ui(ui, |ui| {
                    for profile in &settings.profiles {
                        if ui
                            .selectable_label(
                                profile.name == settings.default_profile,
                                &profile.name,
                            )
                            .clicked()
                        {
                            settings.default_profile = profile.name.clone();
                            changed = true;
                        }
                    }
                });
        }
    });

    changed
}

/// What the instance picker shows when it is closed.
///
/// The endpoint rather than the bare name when the two differ: a profile
/// pointing at a remote server names the server in `instance`, and the address
/// is the part that says it is not the local instance of the same name.
fn instance_label(profile: &Profile) -> String {
    match profile.remote.as_ref() {
        Some(remote) => format!("{}  ({})", profile.instance, remote.address),
        None if profile.instance.is_empty() => tr("Choose...").to_string(),
        None => profile.instance.clone(),
    }
}

/// The instance picker's contents: the same list the `+` button's right-click
/// menu offers, because it is the same question - which server or instance does
/// this profile connect to.
///
/// Returns true when a choice was made.
fn instance_menu(
    ui: &mut Ui,
    profile: &mut Profile,
    instances: &[String],
    servers: &ServerList,
) -> bool {
    let mut changed = false;

    if !servers.is_empty() {
        ui.weak(tr("IRIS servers"));
        for server in &servers.servers {
            let selected = profile.instance.eq_ignore_ascii_case(&server.name);
            let entry = ui.selectable_label(selected, server.menu_label());
            // What the entry does, since a local server opens an instance and
            // a remote one is a Telnet login.
            let hint = match server.target(instances) {
                Target::Local { instance } => tr1("Local session on instance {}", &instance),
                Target::Telnet { address, port } => {
                    tr1("Telnet login to {}", &format!("{address}:{port}"))
                }
            };
            if entry.on_hover_text(hint).clicked() {
                point_at(profile, server, instances);
                changed = true;
            }
        }
        let loose = instances
            .iter()
            .any(|name| !servers.covers_instance(instances, name));
        if loose {
            ui.separator();
        }
    }

    for name in instances {
        // Skipped when a server entry already opens it: the same session under
        // two names is not a choice.
        if servers.covers_instance(instances, name) {
            continue;
        }
        let selected = profile.remote.is_none() && &profile.instance == name;
        if ui.selectable_label(selected, name).clicked() {
            point_at(profile, &Server::for_instance(name), instances);
            changed = true;
        }
    }

    changed
}

/// Points a profile at one of the launcher's servers, exactly as the `+` menu
/// does: the name is what the tab will say, and the target decides whether the
/// session starts locally or logs in over Telnet.
fn point_at(profile: &mut Profile, server: &Server, instances: &[String]) {
    match server.target(instances) {
        Target::Local { instance } => {
            profile.instance = instance;
            profile.remote = None;
        }
        Target::Telnet { address, port } => {
            profile.instance = server.name.clone();
            profile.remote = Some(Remote { address, port });
        }
    }
}

fn labelled_edit(ui: &mut Ui, label: &'static str, value: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr(label));
        changed = ui.text_edit_singleline(value).changed();
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::macros::Param;

    fn macro_with_params() -> Macro {
        Macro {
            name: "Show global".into(),
            params: vec![Param {
                name: "g".into(),
                prompt: "Global".into(),
                default: "CSW1".into(),
            }],
            body: vec!["ZWRITE ^{{g}}".into()],
            ..Macro::default()
        }
    }

    #[test]
    fn a_pending_macro_starts_from_its_defaults() {
        let pending = PendingMacro::new(macro_with_params());
        assert_eq!(pending.values, vec![("g".to_string(), "CSW1".to_string())]);
        assert_eq!(pending.preview(), vec!["ZWRITE ^CSW1".to_string()]);
    }

    #[test]
    fn editing_a_value_changes_the_preview() {
        let mut pending = PendingMacro::new(macro_with_params());
        pending.values[0].1 = "OTHER".into();
        assert_eq!(pending.preview(), vec!["ZWRITE ^OTHER".to_string()]);
    }

    /// A parameterless destructive macro must still stop for a yes/no.
    #[test]
    fn a_confirming_macro_without_params_goes_straight_to_confirmation() {
        let pending = PendingMacro::new(Macro {
            name: "Kill".into(),
            confirm: true,
            body: vec!["KILL ^DATA".into()],
            ..Macro::default()
        });
        assert!(pending.confirming);
    }

    #[test]
    fn a_harmless_macro_does_not_ask_for_confirmation() {
        let pending = PendingMacro::new(Macro {
            body: vec!["Write 1".into()],
            ..Macro::default()
        });
        assert!(!pending.confirming);
        assert!(!pending.source.confirm);
    }
}
