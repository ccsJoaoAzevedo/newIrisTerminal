//! Side panels and dialogs: the IRIS utilities, settings, and the dialogs the
//! rest of the interface hands its questions to.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.
//!
//! Three things used to live here and no longer do. Macros are managed in
//! [`crate::ui::macro_manager`], a window of its own reached from Settings, and
//! run from the terminal's right-click menu; exporting and the IRIS utilities
//! are on that menu too, beside the output they act on. What is left of the
//! macros here is the confirmation step, which is the part that has to
//! interrupt.

use egui::{Context, Ui};

use crate::config::profile::Remote;
use crate::config::servers::{Server, ServerList, Target};
use crate::config::{profile::LogMode, CursorStyle, Profile, Settings, Theme};
use crate::features::macros::Macro;
use crate::features::natives::Native;
use crate::i18n::{tr, tr1, tr2};
use crate::term::Encoding;

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
    /// Store (or, when empty, forget) the password for the HTTP proxy.
    SetProxyPassword(String),
}

/// State the panels own between frames.
#[derive(Default)]
pub struct PanelState {
    pub show_settings: bool,
    /// The theme manager, which keeps its own selection and rename draft.
    pub themes: crate::ui::theme_manager::ThemeManagerState,
    /// The macro manager, which keeps its own selection and editing draft.
    pub macros: crate::ui::macro_manager::MacroManagerState,
    /// A macro waiting on parameter values and/or confirmation.
    pub pending: Option<PendingMacro>,
    /// An IRIS helper waiting on its fields.
    pub pending_native: Option<PendingNative>,
    /// The proxy password as it is being typed. Handed to the credential store
    /// when the field loses focus and cleared immediately, so the secret is not
    /// left sitting in the app's state for the rest of the session.
    proxy_password: String,
    /// The settings window is listening for the manager's chord.
    ///
    /// Its own flag rather than the manager's: both pickers consume the key
    /// presses of the frame they are listening in, and sharing one would have
    /// the manager's editor - drawn later in the frame - record the chord meant
    /// for the setting into whichever macro happened to be open. Public for the
    /// same reason the manager's is: the app has to stop claiming shortcuts for
    /// itself while it is set.
    pub capture_manager_shortcut: bool,
    /// Installed monospace families, listed once. Enumerating system fonts is
    /// slow enough that doing it per frame would be felt while the Settings
    /// window is open.
    font_families: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct PendingMacro {
    pub source: Macro,
    pub values: Vec<(String, String)>,
    /// Set once parameters are filled and only the yes/no remains.
    pub confirming: bool,
    /// The keyboard has yet to be put in the first field. See `focus_first`.
    focus_first: bool,
}

impl PendingMacro {
    pub fn new(source: Macro) -> Self {
        let values = source.default_values();
        let confirming = values.is_empty() && source.confirm;
        PendingMacro {
            source,
            values,
            confirming,
            focus_first: true,
        }
    }

    /// A macro whose parameters have already been filled in — by the
    /// right-click menu's own fields — and which only wants the yes/no its
    /// `confirm` flag asks for.
    pub fn to_confirm(source: Macro, values: Vec<(String, String)>) -> Self {
        PendingMacro {
            source,
            values,
            confirming: true,
            // Nothing to type into: the values are already filled in and the
            // only thing left is the yes/no.
            focus_first: false,
        }
    }

    pub fn preview(&self) -> Vec<String> {
        self.source.expand(&self.values)
    }
}

/// Dims everything behind a dialog, and swallows the clicks aimed at it.
///
/// egui 0.28 has no modal of its own, and these two dialogs have to be modal:
/// both of them are the last look at a line of ObjectScript before it reaches a
/// shared `RDB*` database, and a dialog you can click straight past while it is
/// open is one you can answer by accident. The veil is drawn in the same layer
/// order as a window and before the window, so it covers the terminal and the
/// window covers it.
fn veil(ctx: &Context, id: &str) {
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new((id, "veil")))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            // Allocated before it is painted, because an area's painter is
            // clipped to what the area has claimed - and the click-and-drag
            // sense is the half that makes it modal rather than decorative.
            ui.allocate_response(screen.size(), egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(110));
        });
}

/// The frame both dialogs are drawn in: veiled, centred, and not resizable.
///
/// Centred by anchor rather than by opening position, so it cannot be dragged
/// off to a corner and then be somewhere else the next time. A dialog answering
/// one question does not need to be moved.
fn modal<R>(
    ctx: &Context,
    id: &str,
    title: String,
    open: &mut bool,
    contents: impl FnOnce(&mut Ui) -> R,
) {
    veil(ctx, id);
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(open)
        .show(ctx, contents);
}

/// Puts the keyboard in the first field of a dialog that has just opened.
///
/// Without it the terminal keeps the keyboard - it claims it back whenever no
/// other widget holds it - so the first thing typed into a freshly opened
/// dialog went to the IRIS prompt behind it instead. `first` is the response of
/// the first field; `pending` is cleared once it has been given focus, so
/// clicking into a later field is not undone on the next frame.
fn focus_first(first: &egui::Response, pending: &mut bool) {
    if *pending {
        first.request_focus();
        *pending = false;
    }
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

    modal(ctx, "nit-macro-run", title, &mut open, |ui| {
        ui.set_min_width(360.0);
        if !pending.source.description.is_empty() {
            ui.label(&pending.source.description);
            ui.separator();
        }

        for (index, param) in pending.source.params.iter().enumerate() {
            ui.horizontal(|ui| {
                let prompt = if param.prompt.is_empty() {
                    &param.name
                } else {
                    &param.prompt
                };
                ui.label(prompt);
                if let Some((_, value)) = pending.values.get_mut(index) {
                    let field = ui.add(
                        egui::TextEdit::singleline(value)
                            .desired_width(f32::INFINITY)
                            .hint_text(&param.default),
                    );
                    if index == 0 {
                        focus_first(&field, &mut pending.focus_first);
                    }
                }
            });
        }

        ui.separator();
        ui.label(tr("Will send:"));
        let preview = pending.source.expand(&pending.values);
        // Hidden bodies stay hidden even here: the flag exists because the
        // body carries a credential, and this dialog is on screen.
        if pending.source.hide_command {
            ui.weak(tr("Hidden; this macro carries a secret."));
        } else {
            for line in &preview {
                ui.code(line);
            }
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

/// An IRIS helper waiting for its fields to be filled in.
#[derive(Clone, Debug)]
pub struct PendingNative {
    pub source: Native,
    pub values: Vec<String>,
    /// The keyboard has yet to be put in the first field. See [`focus_first`].
    focus_first: bool,
}

impl PendingNative {
    pub fn new(source: Native) -> Self {
        PendingNative {
            source,
            values: source.default_values(),
            focus_first: true,
        }
    }
}

/// Field-fill dialog for an IRIS helper.
///
/// The composed line is always shown, and that is the point of the dialog
/// rather than a nicety: a helper's arguments are positional and quoted, so
/// reading the call back is the only way to see that the package name landed in
/// the argument meant for it.
pub fn pending_native_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    let pending = state.pending_native.as_mut()?;

    let mut request = None;
    let mut close = false;
    let mut open = true;
    let native = pending.source;

    // A selection made before a reload could be holding fewer values than the
    // helper now asks for.
    pending.values.resize(native.params().len(), String::new());

    modal(
        ctx,
        "nit-native-run",
        tr(native.label()).to_string(),
        &mut open,
        |ui| {
            ui.set_min_width(360.0);
            for (index, param) in native.params().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(tr(param.label));
                    if let Some(value) = pending.values.get_mut(index) {
                        let field =
                            ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                        if index == 0 {
                            focus_first(&field, &mut pending.focus_first);
                        }
                    }
                });
            }

            let invocation = native.build(&pending.values);
            ui.separator();
            ui.label(tr("Will send:"));
            for line in &invocation.lines {
                ui.code(line);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("Send")).clicked() {
                    request = Some(UiRequest::RunNative(native, pending.values.clone()));
                    close = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
                if ui
                    .button(tr("Reset"))
                    .on_hover_text(tr("Back to the fields this helper starts with."))
                    .clicked()
                {
                    pending.values = native.default_values();
                }
            });
        },
    );

    if close || !open {
        state.pending_native = None;
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
                // The way out when the app cannot get the file itself. On a
                // network whose proxy demands NTLM the download can never
                // succeed - ureq speaks only Basic - and the browser, which
                // authenticates as the logged-in user, always can. Offered
                // only once something has gone wrong, so the ordinary path
                // stays one press.
                ui.horizontal(|ui| {
                    ui.small(tr("Or fetch it yourself:"));
                    ui.hyperlink_to(tr("open the download"), &release.download);
                });
                ui.small(tr(
                    "Save it next to the running program, then replace the program with it.",
                ));
            }
            ui.add_space(6.0);
            ui.separator();

            if state.downloading {
                // How far, not just that it is trying. A twelve-megabyte
                // executable through a corporate proxy takes long enough that
                // a bare spinner leaves no way to tell a slow download from a
                // stuck one - which is the state this updater was reported in.
                let (done, total) = state.downloaded();
                ui.horizontal(|ui| {
                    ui.spinner();
                    if total > 0 {
                        ui.label(tr2(
                            "Downloading... {} of {}",
                            &megabytes(done),
                            &megabytes(total),
                        ));
                    } else {
                        ui.label(tr1("Downloading... {}", &megabytes(done)));
                    }
                });
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(260.0)
                            .show_percentage(),
                    );
                }
                return;
            }
            ui.horizontal(|ui| {
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

/// A byte count as megabytes, for a download whose size is the only thing worth
/// saying about it.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
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
                if ui
                    .checkbox(
                        &mut settings.tabs_in_title_bar,
                        tr("Tabs on window title bar"),
                    )
                    .on_hover_text(tr(
                        "Puts the tabs on the same row as the window buttons, from the new-session button across to the gear. One row instead of two; the session line - instance, PID and size - goes, since the tabs already say which session it is.",
                    ))
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
                    // Only shown when there is a proxy to authenticate to. A
                    // proxy that lets the check through and then demands
                    // credentials for the host GitHub serves the release from
                    // is what left this updater checking successfully and
                    // never downloading, so the fields are here rather than
                    // the failure being something only the log knows about.
                    ui.horizontal(|ui| {
                        ui.label(tr("Proxy user"));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut settings.proxy_user)
                                    .desired_width(140.0),
                            )
                            .on_hover_text(tr(
                                "Only if the proxy asks for credentials. Leave empty otherwise.",
                            ))
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(tr("Password"));
                        // Not read back out of the credential store to fill
                        // this in: a password manager is not a place to show
                        // passwords from. Typing here replaces what is stored,
                        // and emptying the field forgets it.
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut state.proxy_password)
                                    .password(true)
                                    .hint_text(if crate::features::update::has_proxy_password() {
                                        tr("stored")
                                    } else {
                                        tr("none")
                                    })
                                    .desired_width(140.0),
                            )
                            .on_hover_text(tr(
                                "Kept in the operating system's credential store, never in settings.toml.",
                            ))
                            .lost_focus()
                        {
                            action = Some(UiRequest::SetProxyPassword(state.proxy_password.clone()));
                            state.proxy_password.clear();
                        }
                    });
                    ui.small(tr(
                        "Basic authentication only. A proxy that insists on NTLM cannot be reached this way; download the release from the browser instead.",
                    ));
                } else {
                    ui.small(tr("No system proxy configured; connecting directly."));
                }

                section(ui, tr("Macros"));
                ui.horizontal(|ui| {
                    // The manager first: it is what anyone opening this section
                    // came for, and the shared file below it is a path most
                    // installs set once and never look at again.
                    if ui
                        .button(tr("Manage macros..."))
                        .on_hover_text(tr("Make, edit and delete your own macros, and read the organization's. Running them is on the terminal's right-click menu."))
                        .clicked()
                    {
                        state.macros.open = true;
                    }
                    if ui
                        .button(tr("Open folder"))
                        .on_hover_text(crate::config::personal_macros_path().display().to_string())
                        .clicked()
                    {
                        if let Some(folder) = crate::config::personal_macros_path().parent() {
                            action = Some(UiRequest::OpenFolder(folder.to_path_buf()));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Shortcut for the manager"));
                    // The same field a macro's own binding is set in, so the
                    // two behave the same way - including the warning when the
                    // chord is one the app has already taken.
                    if crate::ui::shortcut::picker(
                        ui,
                        &mut settings.macro_manager_shortcut,
                        &mut state.capture_manager_shortcut,
                    ) {
                        changed = true;
                    }
                });
                ui.label(tr("Organization macro file (shared, read-only)"));
                let mut org = settings.org_macros_path.display().to_string();
                if ui.text_edit_singleline(&mut org).changed() {
                    settings.org_macros_path = std::path::PathBuf::from(org.trim());
                    changed = true;
                }
                ui.small(tr(
                    "A UNC share, mapped drive, or local copy. Leave empty for none.",
                ));
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

                section(ui, tr("Shells"));
                ui.small(tr(
                    "Other command interpreters, offered under Shells in the new-session menu. Every one of them is a .toml file in the folder below - the ones found installed on this machine were written there for you, and can be renamed, re-armed or deleted like any other.",
                ));
                let shells = crate::plugins::shells::available();
                if shells.is_empty() {
                    ui.weak(tr("None found."));
                }
                for shell in &shells {
                    ui.horizontal(|ui| {
                        ui.label(&shell.name);
                        ui.weak(tr(shell.source_label()));
                    });
                    ui.small(shell.command_line());
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(tr("Open folder"))
                        .on_hover_text(crate::plugins::shells::shells_dir().display().to_string())
                        .clicked()
                    {
                        action = Some(UiRequest::OpenFolder(
                            crate::plugins::shells::shells_dir(),
                        ));
                    }
                    if ui
                        .button(tr("Reload"))
                        .on_hover_text(tr("Probe again and re-read the folder."))
                        .clicked()
                    {
                        // The list is cached for the process, because the
                        // new-session menu asks for it on every frame it is
                        // open. This is the way to say a file has just changed.
                        crate::plugins::shells::refresh();
                    }
                });

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
