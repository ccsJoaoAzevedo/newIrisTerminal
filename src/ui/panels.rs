//! Side panels and dialogs: macros, natives, settings, and export.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.

use egui::{Context, Ui};

use crate::config::{profile::LogMode, Profile, Settings, Theme};
use crate::features::macros::{Macro, MacroGroup};
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

/// The macro browser. Returns a request when the user picks something.
pub fn macros_panel(
    ui: &mut Ui,
    groups: &[MacroGroup],
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
        ui.small("Add them to macros.xml in the config folder, then press Reload.");
        return request;
    }

    let filter = state.macro_filter.to_lowercase();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for group in groups {
            let matching: Vec<&Macro> = group
                .macros
                .iter()
                .filter(|m| {
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
                .default_open(true)
                .show(ui, |ui| {
                    for m in matching {
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
                                request = Some(UiRequest::RunMacro(m.clone()));
                            }
                            if let Some(key) = &m.key {
                                ui.weak(key);
                            }
                        });
                    }
                });
        }
    });

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
