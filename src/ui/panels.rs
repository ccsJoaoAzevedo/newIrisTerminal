//! Side panels and dialogs: the IRIS utilities, settings, and the dialogs the
//! rest of the interface hands its questions to.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.
//!
//! Three things used to live here and no longer do. Macros are managed on
//! the Macros page of Settings ([`crate::ui::settings_view::macros`]), and
//! run from the terminal's right-click menu; exporting and the IRIS utilities
//! are on that menu too, beside the output they act on. What is left of the
//! macros here is the confirmation step, which is the part that has to
//! interrupt.

use crate::ui::dialog::{self, Role};
use egui::{Context, Ui};

use crate::features::macros::Macro;
use crate::features::natives::Native;
use crate::i18n::{tr, tr1, tr2};

/// Colour for "this is set, but it will not do what you expect". Not from the
/// theme: it has to stay legible as a warning in every one of them.
pub const WARNING: egui::Color32 = egui::Color32::from_rgb(220, 120, 60);

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
    /// Keep the main window above all others, or stop.
    ToggleAlwaysOnTop,
    /// Write the personal macro file back to disk.
    SavePersonalMacros,
    /// Show a folder in the platform's file manager.
    OpenFolder(std::path::PathBuf),
    /// Ask GitHub whether there is a newer version, and say either way.
    CheckForUpdates,
    /// Store (or, when empty, forget) the password for the HTTP proxy.
    SetProxyPassword(String),
    /// Something the Themes pages asked for: using, writing or deleting a
    /// theme, or drawing the window again in one edited in place.
    Theme(crate::ui::settings_view::themes::ThemeAction),
    /// Run this screen saver over the whole window now, until the next key or
    /// movement.
    PreviewScreensaver(crate::features::screensaver::Config),
    /// Open Settings on this page - the way into what used to be a manager's
    /// window of its own.
    OpenSettings(crate::ui::settings_view::Route),
    /// Put the usage report into an e-mail to this address, in the user's
    /// own mail program.
    SendUsageReport(String),
    /// Stop offering the usage report after updates.
    StopAskingUsageReport,
}

/// State the panels own between frames.
#[derive(Default)]
pub struct PanelState {
    pub show_settings: bool,
    /// The page the settings window is on, kept while it is closed so it
    /// reopens there.
    pub settings_route: crate::ui::settings_view::Route,
    /// What is typed in the settings window's search field.
    pub settings_search: String,
    /// The row a search result led to, and when, so it can be lit briefly.
    pub settings_highlight: Option<(&'static str, f64)>,
    /// The row to scroll into view on the next frame the page is drawn.
    pub settings_scroll_to: Option<&'static str>,
    /// The Themes pages, which keep their own selection and rename draft.
    pub themes: crate::ui::settings_view::themes::ThemePageState,
    /// The Macros pages, which keep their own selection and editing draft.
    pub macros: crate::ui::settings_view::macros::MacroPageState,
    /// The Screen saver page's little monitor and file dialog.
    pub screensaver: crate::ui::settings_view::screensaver::SaverPageState,
    /// A macro waiting on parameter values and/or confirmation.
    pub pending: Option<PendingMacro>,
    /// An IRIS helper waiting on its fields.
    pub pending_native: Option<PendingNative>,
    /// The usage report, offered and waiting on an answer.
    pub usage: Option<UsageOffer>,
    /// The proxy password as it is being typed. Handed to the credential store
    /// when the field loses focus and cleared immediately, so the secret is not
    /// left sitting in the app's state for the rest of the session.
    pub(crate) proxy_password: String,
    /// The Macros page is listening for the chord that opens it.
    ///
    /// Its own flag rather than the macro editor's: both pickers consume the
    /// key presses of the frame they are listening in, and sharing one would
    /// have the editor record the chord meant for the setting into whichever
    /// macro happened to be open. Public for the same reason the editor's is:
    /// the app has to stop claiming shortcuts for itself while it is set.
    pub capture_manager_shortcut: bool,
    /// Installed monospace families, listed once. Enumerating system fonts is
    /// slow enough that doing it per frame would be felt while the Settings
    /// window is open.
    pub(crate) font_families: Option<Vec<String>>,
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

/// Moves the keyboard on when Enter is pressed in a dialog field.
///
/// Enter in the last field runs the dialog instead, which is the whole point:
/// a macro with one parameter is then type-value-Enter, without reaching for
/// the mouse. Returns true when the dialog should be submitted.
///
/// `fields` is one response per field, in the order they were drawn. Tab
/// already walks them; this makes Enter do the same thing, because a dialog
/// that is a list of values to fill in is one people finish with Enter.
fn advance_on_enter(ui: &Ui, fields: &[egui::Response]) -> bool {
    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if !entered {
        return false;
    }
    // `lost_focus` rather than `has_focus`: a single-line field surrenders the
    // keyboard the moment Enter reaches it, so by the time this runs the field
    // that was typed into no longer holds focus - it is the one that just gave
    // it up. Nothing else takes the keyboard away on the same frame as an
    // Enter press.
    let Some(at) = fields.iter().position(|f| f.lost_focus() || f.has_focus()) else {
        // Enter with the keyboard somewhere else - on a button, say - is that
        // widget's own business.
        return false;
    };
    match fields.get(at + 1) {
        Some(next) => {
            next.request_focus();
            false
        }
        None => true,
    }
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

    dialog::show(ctx, "nit-macro-run", &title, &mut open, |ui| {
        ui.set_min_width(360.0);
        if !pending.source.description.is_empty() {
            ui.vertical_centered(|ui| ui.weak(&pending.source.description));
        }

        // Only the parameters that reach the command, the same ones `values`
        // was built from - so the two line up index for index.
        let mut fields = Vec::with_capacity(pending.values.len());
        for (index, param) in pending.source.usable_params().enumerate() {
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
                    fields.push(field);
                }
            });
        }
        let submitted = advance_on_enter(ui, &fields);

        ui.add_space(4.0);
        ui.weak(tr("Will send:"));
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
            ui.colored_label(
                WARNING,
                tr("This macro is marked as modifying data. RDB* databases are shared with the team."),
            );
        }

        dialog::actions(ui, |ui| {
            // A macro marked as modifying data is answered with the red
            // button, as a deletion is on either desktop.
            let (send_label, role) = if pending.source.confirm {
                (tr("Yes, send it"), Role::Destructive)
            } else {
                (tr("Send"), Role::Suggested)
            };
            let send = dialog::button(ui, send_label, role);
            // Enter in the last field sends - except on a macro marked as
            // modifying data, where it only moves the keyboard onto the button.
            // The yes/no is there to be answered deliberately, and a second
            // Enter is deliberate in a way that finishing a field is not.
            if send.clicked() || (submitted && !pending.source.confirm) {
                request = Some(UiRequest::SendLines(preview.clone()));
                close = true;
            } else if submitted {
                send.request_focus();
            }
            if dialog::button(ui, tr("Cancel"), Role::Plain).clicked() {
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

    dialog::show(ctx, "nit-native-run", tr(native.label()), &mut open, |ui| {
        ui.set_min_width(360.0);
        let mut fields = Vec::with_capacity(native.params().len());
        for (index, param) in native.params().iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(tr(param.label));
                if let Some(value) = pending.values.get_mut(index) {
                    let field =
                        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                    if index == 0 {
                        focus_first(&field, &mut pending.focus_first);
                    }
                    fields.push(field);
                }
            });
        }
        let submitted = advance_on_enter(ui, &fields);

        let invocation = native.build(&pending.values);
        ui.add_space(4.0);
        ui.weak(tr("Will send:"));
        for line in &invocation.lines {
            ui.code(line);
        }

        dialog::actions(ui, |ui| {
            if dialog::button(ui, tr("Send"), Role::Suggested).clicked() || submitted {
                request = Some(UiRequest::RunNative(native, pending.values.clone()));
                close = true;
            }
            if dialog::button(ui, tr("Cancel"), Role::Plain).clicked() {
                close = true;
            }
            // Furthest left, away from the answer, as a secondary action
            // sits on either desktop.
            ui.add_space(12.0);
            if dialog::button(ui, tr("Reset"), Role::Plain)
                .on_hover_text(tr("Back to the fields this helper starts with."))
                .clicked()
            {
                pending.values = native.default_values();
            }
        });
    });

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
/// The usage report, offered after an update: what it says, and who it is to
/// go to.
#[derive(Clone, Debug)]
pub struct UsageOffer {
    /// The report as it will be sent, shown in full before it is.
    pub report: String,
    /// The address, as it is being typed.
    pub email: String,
}

/// Asks whether to send the usage report, and to whom.
///
/// The address is typed by the user every time it is not already set: the
/// report goes to whoever the person sending it chooses, and never anywhere
/// by default.
pub fn usage_report_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    let offer = state.usage.as_mut()?;
    let mut open = true;
    let mut request = None;
    let mut dismissed = false;
    dialog::show(
        ctx,
        "nit-usage-report",
        tr("Help decide what to keep"),
        &mut open,
        |ui| {
            ui.set_max_width(460.0);
            ui.label(tr(
            "Sending this report says which options you use differently from a fresh install, so the ones nobody uses can go and the ones everybody changes can become the default. It is put into an e-mail in your own mail program, for you to send.",
        ));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(tr("Send to"));
                ui.add(
                    egui::TextEdit::singleline(&mut offer.email)
                        .hint_text(tr("name@company.com"))
                        .desired_width(260.0),
                );
            });
            egui::CollapsingHeader::new(tr("What is sent"))
                .id_source("nit-usage-report-preview")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&offer.report).monospace().small(),
                                )
                                .selectable(true),
                            );
                        });
                });
            ui.weak(tr(
            "No server addresses, user names, paths or command history. Profiles, macros and themes are only counted.",
        ));
            let can_send = crate::features::usage::plausible_address(&offer.email);
            dialog::actions(ui, |ui| {
                let send = ui.add_enabled_ui(can_send, |ui| {
                    dialog::button(ui, tr("Send"), Role::Suggested)
                });
                if send.inner.clicked() {
                    request = Some(UiRequest::SendUsageReport(offer.email.trim().to_string()));
                }
                if dialog::button(ui, tr("Not now"), Role::Plain).clicked() {
                    dismissed = true;
                }
                if dialog::button(ui, tr("Don't ask again"), Role::Plain)
                    .on_hover_text(tr("Settings, About, still sends it whenever you want."))
                    .clicked()
                {
                    request = Some(UiRequest::StopAskingUsageReport);
                }
            });
        },
    );
    if !open || dismissed || request.is_some() {
        state.usage = None;
    }
    request
}

pub fn update_dialog(ctx: &Context, state: &mut crate::app::UpdateState) -> bool {
    let Some(release) = state.available.clone() else {
        return false;
    };
    if !state.asked {
        return false;
    }
    let mut apply = false;
    let mut open = true;

    dialog::show(ctx, "nit-update", tr("Update available"), &mut open, |ui| {
        {
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
            dialog::actions(ui, |ui| {
                if state.staged.is_some() {
                    if dialog::button(ui, tr("Restart and update"), Role::Suggested)
                        .on_hover_text(tr(
                            "Puts the new version in place and starts it. Closes straight away, without asking about connected sessions - this is the close you just asked for. Each of them is sent HALT on the way out.",
                        ))
                        .clicked()
                    {
                        apply = true;
                    }
                } else if dialog::button(ui, tr("Download"), Role::Suggested).clicked() {
                    state.start_download();
                }
                if dialog::button(ui, tr("Later"), Role::Plain).clicked() {
                    // Dismissed for this run. The next start asks again, which
                    // is the whole of the nagging this does.
                    state.asked = false;
                }
            });
        }
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

pub use crate::ui::settings_view::settings_dialog;

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

    /// A parameter that cannot reach the command must not turn a confirming
    /// macro back into a fill-in form with a field that goes nowhere.
    #[test]
    fn a_confirming_macro_with_only_unusable_params_goes_straight_to_confirmation() {
        let pending = PendingMacro::new(Macro {
            confirm: true,
            params: vec![
                Param::default(),
                Param {
                    name: "unused".into(),
                    ..Param::default()
                },
            ],
            body: vec!["KILL ^DATA".into()],
            ..Macro::default()
        });
        assert!(pending.confirming);
        assert!(pending.values.is_empty());
    }

    #[test]
    fn the_dialog_holds_values_only_for_the_params_it_will_ask_for() {
        let mut m = macro_with_params();
        m.params.insert(0, Param::default());
        let pending = PendingMacro::new(m);
        assert_eq!(pending.values, vec![("g".to_string(), "CSW1".to_string())]);
        assert_eq!(pending.preview(), vec!["ZWRITE ^CSW1".to_string()]);
    }
}
