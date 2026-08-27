//! Side panels and dialogs: macros, natives, settings, and export.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.

use egui::{Context, Ui};

use crate::config::{profile::LogMode, Profile, Settings, Theme};
use crate::features::global_browser::{Node, Page, Query};
use crate::features::macros::{Macro, MacroGroup, Origin, Param};
use crate::features::natives::Native;
use crate::term::Encoding;

/// Something the user asked for. `app.rs` decides whether and how to honour it.
#[derive(Clone, Debug)]
pub enum UiRequest {
    /// Send a macro, after confirmation if it asks for one.
    RunMacro(Macro),
    /// Send a native-utility invocation.
    RunNative(Native, String),
    /// Send literal lines to the active session.
    SendLines(Vec<String>),
    ExportText(crate::features::export::Range),
    ExportHtml(crate::features::export::Range),
    CopyRange(crate::features::export::Range),
    SettingsChanged,
    ReloadMacros,
    /// Write the personal macro file back to disk.
    SavePersonalMacros,
    /// Run the global browser's query against the active session.
    RunGlobalQuery,
    /// Copy the visible global-browser rows to the clipboard.
    CopyGlobalPage,
}

/// State the panels own between frames.
#[derive(Default)]
pub struct PanelState {
    pub show_macros: bool,
    pub show_settings: bool,
    pub show_export: bool,
    pub macro_filter: String,
    /// A macro waiting on parameter values and/or confirmation.
    pub pending: Option<PendingMacro>,
    /// Free-text argument for the selected native helper.
    pub native_arg: String,
    pub selected_native: Option<Native>,
    /// The personal macro currently open in the editor, by (group, index).
    pub editing: Option<(usize, usize)>,
    /// Draft being edited, kept separate so Cancel is a real cancel.
    pub draft: Option<Macro>,
    /// Group name for a macro about to be created.
    pub new_group: String,
    pub show_globals: bool,
    pub globals: GlobalBrowserState,
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

/// The macro browser.
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

    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.text_edit_singleline(&mut state.macro_filter);
        if ui.button("Reload").clicked() {
            request = Some(UiRequest::ReloadMacros);
        }
    });
    ui.separator();

    if groups.is_empty() {
        ui.label("No macros defined.");
        ui.small("Add them below, or configure the organization file in Settings.");
    }

    let filter = state.macro_filter.to_lowercase();
    let mut to_run: Option<Macro> = None;
    let mut to_edit: Option<(usize, usize)> = None;
    let mut to_delete: Option<(usize, usize)> = None;

    egui::ScrollArea::vertical()
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
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
                                let label = if m.confirm {
                                    format!("{}  (confirms)", m.name)
                                } else {
                                    m.name.clone()
                                };
                                let button = ui.button(label);
                                let button = if m.description.is_empty() {
                                    button
                                } else {
                                    button.on_hover_text(&m.description)
                                };
                                if button.clicked() {
                                    to_run = Some(m.clone());
                                }

                                // Provenance at a glance: a shared macro
                                // behaving oddly is someone else's file, not
                                // something the user can have broken locally.
                                match m.origin {
                                    Origin::Organization => {
                                        ui.weak("org").on_hover_text(
                                            "Provided by the organization; read-only here.",
                                        );
                                    }
                                    Origin::Personal => {
                                        if ui.small_button("edit").clicked() {
                                            to_edit = Some((gi, mi));
                                        }
                                        if ui.small_button("x").on_hover_text("Delete").clicked() {
                                            to_delete = Some((gi, mi));
                                        }
                                    }
                                }
                            });
                        }
                    });
            }
        });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("New in group");
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
            state.editing = Some((gi, mi));
            state.draft = Some(groups[gi].macros[mi].clone());
        }
    });

    if let Some((gi, mi)) = to_delete {
        // Guarded by construction — delete only appears on personal macros —
        // but checked anyway so a future refactor cannot destroy shared data.
        if groups[gi].macros[mi].origin.is_editable() {
            groups[gi].macros.remove(mi);
            if groups[gi].macros.is_empty() {
                groups.remove(gi);
            }
            state.editing = None;
            state.draft = None;
            request = Some(UiRequest::SavePersonalMacros);
        }
    }

    if let Some((gi, mi)) = to_edit {
        state.editing = Some((gi, mi));
        state.draft = Some(groups[gi].macros[mi].clone());
    }

    if let Some(r) = macro_editor(ui, groups, state) {
        request = Some(r);
    }

    if let Some(m) = to_run {
        request = Some(UiRequest::RunMacro(m));
    }
    request
}

/// Inline editor for one personal macro.
fn macro_editor(
    ui: &mut Ui,
    groups: &mut [MacroGroup],
    state: &mut PanelState,
) -> Option<UiRequest> {
    let (gi, mi) = state.editing?;
    if groups.get(gi).and_then(|g| g.macros.get(mi)).is_none() {
        state.editing = None;
        state.draft = None;
        return None;
    }
    let draft = state.draft.as_mut()?;

    let mut request = None;
    let mut close = false;

    ui.separator();
    ui.heading("Edit macro");

    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut draft.name);
    });
    ui.horizontal(|ui| {
        ui.label("Description");
        ui.text_edit_singleline(&mut draft.description);
    });

    ui.checkbox(
        &mut draft.confirm,
        "Confirm before sending (use for anything that writes)",
    );

    ui.label("Body - one command per line, {{param}} is substituted");
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

    ui.label("Parameters");
    let mut drop_param = None;
    for (pi, param) in draft.params.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut param.name).desired_width(60.0))
                .on_hover_text("Name used as {{name}} in the body");
            ui.add(egui::TextEdit::singleline(&mut param.prompt).desired_width(100.0))
                .on_hover_text("Prompt shown when running");
            ui.add(egui::TextEdit::singleline(&mut param.default).desired_width(70.0))
                .on_hover_text("Default value");
            if ui.small_button("x").clicked() {
                drop_param = Some(pi);
            }
        });
    }
    if let Some(pi) = drop_param {
        draft.params.remove(pi);
    }
    if ui.small_button("Add parameter").clicked() {
        draft.params.push(Param::default());
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            // Origin is never taken from the draft: an edited macro stays
            // personal, so nothing can promote itself into the shared file.
            let mut saved = draft.clone();
            saved.origin = Origin::Personal;
            groups[gi].macros[mi] = saved;
            close = true;
            request = Some(UiRequest::SavePersonalMacros);
        }
        if ui.button("Cancel").clicked() {
            close = true;
        }
    });

    if close {
        state.editing = None;
        state.draft = None;
    }
    request
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
        format!("Confirm: {}", pending.source.name)
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
            ui.label("Will send:");
            let preview = pending.source.expand(&pending.values);
            for line in &preview {
                ui.code(line);
            }

            if pending.source.confirm {
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(220, 120, 60),
                    "This macro is marked as modifying data. \
                     RDB* databases are shared with the team.",
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                let send_label = if pending.source.confirm {
                    "Yes, send it"
                } else {
                    "Send"
                };
                if ui.button(send_label).clicked() {
                    request = Some(UiRequest::SendLines(preview.clone()));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
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

    ui.label("IRIS utilities");
    ui.separator();

    for native in Native::ALL {
        let selected = state.selected_native == Some(native);
        if ui.selectable_label(selected, native.label()).clicked() {
            state.selected_native = Some(native);
            state.native_arg.clear();
        }
    }

    if let Some(native) = state.selected_native {
        ui.separator();
        if let Some(prompt) = native.argument_prompt() {
            ui.label(prompt);
            ui.text_edit_singleline(&mut state.native_arg);
        }
        let invocation = native.build(&state.native_arg);
        ui.label("Will send:");
        for line in &invocation.lines {
            ui.code(line);
        }
        if ui.button("Run").clicked() {
            request = Some(UiRequest::RunNative(native, state.native_arg.clone()));
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

    egui::Window::new("Export output")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Save to a file");
            ui.horizontal(|ui| {
                if ui.button("Screen as text").clicked() {
                    request = Some(UiRequest::ExportText(Range::Screen));
                }
                if ui.button("Everything as text").clicked() {
                    request = Some(UiRequest::ExportText(Range::All));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Screen as HTML").clicked() {
                    request = Some(UiRequest::ExportHtml(Range::Screen));
                }
                if ui.button("Everything as HTML").clicked() {
                    request = Some(UiRequest::ExportHtml(Range::All));
                }
            });

            ui.separator();
            ui.label("Copy to the clipboard");
            ui.horizontal(|ui| {
                if ui.button("Screen").clicked() {
                    request = Some(UiRequest::CopyRange(Range::Screen));
                }
                if ui.button("Everything").clicked() {
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

/// Settings: theme, font, scrollback, logging, and profiles.
pub fn settings_dialog(
    ctx: &Context,
    settings: &mut Settings,
    themes: &[Theme],
    state: &mut PanelState,
    instances: &[String],
) -> Option<UiRequest> {
    if !state.show_settings {
        return None;
    }
    let mut changed = false;
    let mut open = true;

    egui::Window::new("Settings")
        .collapsible(false)
        .default_width(520.0)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Appearance");
                ui.horizontal(|ui| {
                    ui.label("Theme");
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
                });
                ui.horizontal(|ui| {
                    ui.label("Font size");
                    if ui
                        .add(egui::Slider::new(&mut settings.font_size, 8.0..=28.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.small("Theme files live in the themes folder; drop one in and restart.");

                ui.separator();
                ui.heading("Session");
                ui.horizontal(|ui| {
                    ui.label("Scrollback lines");
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
                    .checkbox(
                        &mut settings.open_on_start,
                        "Open the default profile at startup",
                    )
                    .changed()
                {
                    changed = true;
                }

                ui.separator();
                ui.heading("Macros");
                ui.label("Organization macro file (shared, read-only)");
                let mut org = settings.org_macros_path.display().to_string();
                if ui.text_edit_singleline(&mut org).changed() {
                    settings.org_macros_path = std::path::PathBuf::from(org.trim());
                    changed = true;
                }
                ui.small(
                    "A UNC share, mapped drive, or local copy. Leave empty for none.                      Your personal macros are edited in the Macros panel.",
                );
                if let Some(path) = settings.org_macros() {
                    if path.exists() {
                        ui.small("Found.");
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 120, 60),
                            "Not reachable right now - personal macros will still load.",
                        );
                    }
                }

                ui.separator();
                ui.heading("Logging");
                ui.horizontal(|ui| {
                    ui.label("Default mode");
                    for (mode, label) in [
                        (LogMode::Off, "Off"),
                        (LogMode::Clean, "Clean text"),
                        (LogMode::Raw, "Raw bytes"),
                    ] {
                        if ui
                            .selectable_label(settings.default_log_mode == mode, label)
                            .clicked()
                        {
                            settings.default_log_mode = mode;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Keep logs for (days, 0 = forever)");
                    if ui
                        .add(egui::DragValue::new(&mut settings.log_retention_days).range(0..=3650))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.small(format!("Logs: {}", settings.log_dir.display()));

                ui.separator();
                ui.heading("Profiles");
                if profiles_editor(ui, settings, instances) {
                    changed = true;
                }
            });
        });

    if !open {
        state.show_settings = false;
    }
    changed.then_some(UiRequest::SettingsChanged)
}

/// Returns true when anything changed.
fn profiles_editor(ui: &mut Ui, settings: &mut Settings, instances: &[String]) -> bool {
    let mut changed = false;
    let mut remove = None;

    for (index, profile) in settings.profiles.iter_mut().enumerate() {
        egui::CollapsingHeader::new(if profile.name.is_empty() {
            format!("Profile {}", index + 1)
        } else {
            profile.name.clone()
        })
        .id_source(("profile", index))
        .show(ui, |ui| {
            changed |= labelled_edit(ui, "Name", &mut profile.name);

            ui.horizontal(|ui| {
                ui.label("Instance");
                if instances.is_empty() {
                    changed |= ui.text_edit_singleline(&mut profile.instance).changed();
                } else {
                    egui::ComboBox::from_id_source(("instance", index))
                        .selected_text(profile.instance.clone())
                        .show_ui(ui, |ui| {
                            for name in instances {
                                if ui
                                    .selectable_label(*name == profile.instance, name)
                                    .clicked()
                                {
                                    profile.instance = name.clone();
                                    changed = true;
                                }
                            }
                        });
                }
            });

            changed |= labelled_edit(ui, "Namespace", &mut profile.namespace);

            ui.horizontal(|ui| {
                ui.label("Encoding");
                egui::ComboBox::from_id_source(("encoding", index))
                    .selected_text(profile.encoding.label())
                    .show_ui(ui, |ui| {
                        for enc in Encoding::ALL {
                            if ui
                                .selectable_label(profile.encoding == enc, enc.label())
                                .clicked()
                            {
                                profile.encoding = enc;
                                changed = true;
                            }
                        }
                    });
            });
            ui.small("Leave as UTF-8 unless accented characters come out wrong.");

            ui.separator();
            changed |= ui
                .checkbox(&mut profile.autologon, "Log in automatically")
                .changed();
            if profile.autologon {
                changed |= labelled_edit(ui, "Username", &mut profile.username);
                password_editor(ui, profile, index);
                if profile.username.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 60),
                        "Autologon needs a username.",
                    );
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Logging");
                for (mode, label) in [
                    (LogMode::Off, "Use default"),
                    (LogMode::Clean, "Clean"),
                    (LogMode::Raw, "Raw"),
                ] {
                    if ui
                        .selectable_label(profile.logging == mode, label)
                        .clicked()
                    {
                        profile.logging = mode;
                        changed = true;
                    }
                }
            });

            if ui.button("Delete this profile").clicked() {
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
        if ui.button("Add profile").clicked() {
            settings.profiles.push(Profile {
                name: format!("Profile {}", settings.profiles.len() + 1),
                instance: instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });
            changed = true;
        }
        if !settings.profiles.is_empty() {
            ui.label("Default");
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

/// Password field. The value is never held in the settings struct — it goes
/// straight to the OS credential store — so this widget keeps its own buffer
/// in egui's temporary memory and clears it once saved.
fn password_editor(ui: &mut Ui, profile: &Profile, index: usize) {
    let id = egui::Id::new(("password-buffer", index));
    let mut buffer: String = ui.memory_mut(|m| m.data.get_temp(id).unwrap_or_default());

    let stored = profile.password().is_some();
    ui.horizontal(|ui| {
        ui.label("Password");
        ui.add(egui::TextEdit::singleline(&mut buffer).password(true));

        if ui.button("Save").clicked() {
            match profile.set_password(&buffer) {
                Ok(()) => buffer.clear(),
                Err(e) => log::error!("could not store the password: {e:#}"),
            }
        }
        if stored && ui.button("Forget").clicked() {
            profile.clear_password();
        }
    });
    ui.small(if stored {
        "A password is stored in the OS credential manager."
    } else {
        "No password stored; autologon will stop at the password prompt."
    });

    ui.memory_mut(|m| m.data.insert_temp(id, buffer));
}

fn labelled_edit(ui: &mut Ui, label: &str, value: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.text_edit_singleline(value).changed();
    });
    changed
}

/// State for the global browser, kept between frames.
pub struct GlobalBrowserState {
    pub query: Query,
    pub page: Page,
    /// Free-text filter applied to the fetched page.
    pub search: String,
    /// Zero-based page index within the fetched rows.
    pub page_index: usize,
    pub rows_per_page: usize,
    /// Set while a query is in flight.
    pub running: bool,
    pub message: Option<String>,
    /// Resume points already visited, so Back walks the global properly
    /// instead of restarting from the top.
    pub history: Vec<String>,
}

impl Default for GlobalBrowserState {
    fn default() -> Self {
        GlobalBrowserState {
            query: Query::default(),
            page: Page::default(),
            search: String::new(),
            page_index: 0,
            rows_per_page: 25,
            running: false,
            message: None,
            history: Vec::new(),
        }
    }
}

impl GlobalBrowserState {
    /// Rows surviving the search filter.
    pub fn filtered(&self) -> Vec<&Node> {
        self.page
            .nodes
            .iter()
            .filter(|n| n.matches(&self.search))
            .collect()
    }

    pub fn page_count(&self) -> usize {
        let total = self.filtered().len();
        total.div_ceil(self.rows_per_page.max(1)).max(1)
    }

    /// Clamps the page index, which the search box can invalidate at any time.
    pub fn clamp_page(&mut self) {
        let last = self.page_count().saturating_sub(1);
        self.page_index = self.page_index.min(last);
    }
}

/// The global browser: query bar, searchable grid, pagination.
pub fn global_browser_panel(
    ui: &mut Ui,
    state: &mut GlobalBrowserState,
    theme: &Theme,
) -> Option<UiRequest> {
    let mut request = None;

    ui.horizontal(|ui| {
        ui.label("Global");
        let editing = ui.add(
            egui::TextEdit::singleline(&mut state.query.global)
                .hint_text("CSW1")
                .desired_width(140.0),
        );
        // Enter in the field runs the query, as in every other query tool.
        let submitted = editing.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.label("Delim");
        ui.add(egui::TextEdit::singleline(&mut state.query.delimiter).desired_width(30.0))
            .on_hover_text("Delimiter for $Piece. ^ is the ObjectScript default.");

        ui.label("Limit");
        ui.add(
            egui::DragValue::new(&mut state.query.limit)
                .range(1..=20_000)
                .speed(10),
        )
        .on_hover_text("Nodes fetched per request");

        let runnable = state.query.is_runnable() && !state.running;
        if (ui.add_enabled(runnable, egui::Button::new("Run")).clicked() || (submitted && runnable))
            && runnable
        {
            state.query.start.clear();
            state.history.clear();
            request = Some(UiRequest::RunGlobalQuery);
        }
    });

    if state.running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading from IRIS...");
        });
    }

    if let Some(error) = &state.page.error {
        ui.colored_label(theme.ansi[9], format!("IRIS error: {error}"));
    }
    if let Some(message) = &state.message {
        ui.small(message);
    }

    if state.page.nodes.is_empty() && !state.running {
        ui.separator();
        ui.small(
            "Reads with $Query/$Get only - nothing here can modify a database. \
             Values are split with $Piece on the IRIS side.",
        );
        return request;
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Search");
        if ui
            .add(
                egui::TextEdit::singleline(&mut state.search)
                    .hint_text("subscript or piece text")
                    .desired_width(180.0),
            )
            .changed()
        {
            state.page_index = 0;
        }
        if ui.small_button("clear").clicked() {
            state.search.clear();
            state.page_index = 0;
        }
    });

    state.clamp_page();
    let sub_cols = state.page.subscript_columns();
    let piece_cols = state.page.piece_columns();
    let rows_per_page = state.rows_per_page.max(1);
    let page_index = state.page_index;

    let matched: Vec<Node> = state.filtered().into_iter().cloned().collect();
    let total = matched.len();
    let start = page_index * rows_per_page;
    let visible: Vec<&Node> = matched.iter().skip(start).take(rows_per_page).collect();

    // The grid scrolls horizontally: a node with twenty pieces is normal in
    // ERP data and must not squeeze the subscripts out of view.
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 40.0)
        .show(ui, |ui| {
            egui::Grid::new("global-grid")
                .striped(true)
                .num_columns(sub_cols + piece_cols + 1)
                .show(ui, |ui| {
                    ui.strong("#");
                    for i in 1..=sub_cols {
                        ui.strong(format!("sub{i}"));
                    }
                    for i in 1..=piece_cols {
                        // Numbered so a column maps directly onto
                        // $Piece(value, delim, n).
                        ui.strong(format!("p{i}"));
                    }
                    ui.end_row();

                    for (offset, node) in visible.iter().enumerate() {
                        let row = start + offset + 1;
                        ui.label(format!("{row}")).on_hover_text(&node.reference);

                        for i in 0..sub_cols {
                            let text = node.subscripts.get(i).cloned().unwrap_or_default();
                            ui.label(text);
                        }
                        for i in 0..piece_cols {
                            match node.pieces.get(i) {
                                Some(piece) if !piece.is_empty() => {
                                    ui.label(piece);
                                }
                                // An empty piece and a missing one are
                                // different facts; show them differently.
                                Some(_) => {
                                    ui.weak("");
                                }
                                None => {
                                    ui.weak("-");
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
        });

    ui.separator();
    ui.horizontal(|ui| {
        let pages = state.page_count();
        if ui
            .add_enabled(state.page_index > 0, egui::Button::new("<"))
            .clicked()
        {
            state.page_index -= 1;
        }
        ui.label(format!(
            "page {}/{}  -  {total} rows",
            state.page_index + 1,
            pages
        ));
        if ui
            .add_enabled(state.page_index + 1 < pages, egui::Button::new(">"))
            .clicked()
        {
            state.page_index += 1;
        }

        ui.separator();

        // Fetching further nodes is a round trip to IRIS, distinct from
        // paging within what has already been fetched.
        if state.page.truncated {
            let more = ui
                .add_enabled(!state.running, egui::Button::new("Fetch more"))
                .on_hover_text("Continue the walk from the last node");
            if more.clicked() {
                if let Some(resume) = state.page.resume_from() {
                    state.history.push(state.query.start.clone());
                    state.query.start = resume;
                    request = Some(UiRequest::RunGlobalQuery);
                }
            }
        }
        if !state.history.is_empty()
            && ui
                .add_enabled(!state.running, egui::Button::new("Back"))
                .clicked()
        {
            if let Some(previous) = state.history.pop() {
                state.query.start = previous;
                request = Some(UiRequest::RunGlobalQuery);
            }
        }

        if ui.button("Copy page").clicked() {
            request = Some(UiRequest::CopyGlobalPage);
        }
    });

    if state.page.truncated {
        ui.small(format!(
            "Stopped at the {}-node limit; more nodes remain.",
            state.query.limit
        ));
    }

    request
}

/// The visible page as tab-separated text, for the clipboard.
pub fn page_as_text(state: &GlobalBrowserState) -> String {
    let sub_cols = state.page.subscript_columns();
    let piece_cols = state.page.piece_columns();

    let mut out = String::new();
    for i in 1..=sub_cols {
        out.push_str(&format!("sub{i}\t"));
    }
    for i in 1..=piece_cols {
        out.push_str(&format!("p{i}"));
        if i < piece_cols {
            out.push('\t');
        }
    }
    out.push('\n');

    for node in state.filtered() {
        for i in 0..sub_cols {
            out.push_str(node.subscripts.get(i).map(String::as_str).unwrap_or(""));
            out.push('\t');
        }
        for i in 0..piece_cols {
            out.push_str(node.pieces.get(i).map(String::as_str).unwrap_or(""));
            if i + 1 < piece_cols {
                out.push('\t');
            }
        }
        out.push('\n');
    }
    out
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
