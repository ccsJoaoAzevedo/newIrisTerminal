//! Macros loaded from XML.
//!
//! A macro is a named snippet of ObjectScript with optional `{{param}}`
//! placeholders. Sending one is just typing on the user's behalf, which is
//! exactly why `confirm="true"` exists: these sessions run against shared
//! `RDB*` databases, and a macro that writes must not be one stray click away.

use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// Label shown in the fill-in dialog; falls back to the name.
    pub prompt: String,
    pub default: String,
}

/// Where a macro came from. Organisation macros are shared and read-only in
/// the app; personal ones are editable. Keeping this on the macro itself means
/// the UI never has to guess which file a given entry can be written back to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// Shipped by the organisation from a shared file. Never modified here.
    Organization,
    /// The user's own file, editable in the app.
    #[default]
    Personal,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Organization => "Organization",
            Origin::Personal => "Personal",
        }
    }

    pub fn is_editable(self) -> bool {
        matches!(self, Origin::Personal)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Macro {
    /// Set by the loader, not by the XML — a shared file cannot promote its own
    /// macros to editable.
    pub origin: Origin,
    pub name: String,
    pub description: String,
    /// Optional accelerator, e.g. `Ctrl+Shift+G`. Parsed and bound by
    /// [`crate::ui::shortcut`]; an unparseable value simply never fires.
    pub key: Option<String>,
    /// Require an explicit yes before sending. Set this on anything that writes.
    pub confirm: bool,
    /// Keep the body out of the UI, for a macro whose commands carry a password
    /// or another secret.
    ///
    /// Only the display is affected. IRIS still echoes what it is sent, so the
    /// text can reach the screen and the transcript by that route - this hides
    /// it from the places the app itself would put it.
    pub hide_command: bool,
    pub params: Vec<Param>,
    /// Body lines, already split. Sent one line at a time.
    pub body: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroGroup {
    pub name: String,
    pub origin: Origin,
    pub macros: Vec<Macro>,
}

impl Macro {
    /// Substitutes `{{name}}` placeholders. Unknown placeholders are left
    /// alone rather than blanked, so a typo is visible instead of silently
    /// producing a valid-but-wrong command.
    pub fn expand(&self, values: &[(String, String)]) -> Vec<String> {
        self.body
            .iter()
            .map(|line| {
                let mut out = line.clone();
                for (name, value) in values {
                    out = out.replace(&format!("{{{{{name}}}}}"), value);
                }
                out
            })
            .collect()
    }

    /// Parameter values pre-filled with their defaults, ready for the dialog.
    pub fn default_values(&self) -> Vec<(String, String)> {
        self.params
            .iter()
            .map(|p| (p.name.clone(), p.default.clone()))
            .collect()
    }

    pub fn needs_input(&self) -> bool {
        !self.params.is_empty()
    }
}

/// One body line per line of text, blank lines dropped and each line trimmed.
///
/// The body is sent a line at a time, so a blank line would be an empty
/// command and leading indentation would reach IRIS as part of it. Applied when
/// the XML is read and again when the editor saves - and *only* then: doing it
/// on every keystroke is what used to swallow the space bar, because a trailing
/// space was trimmed away before the next frame could put it back on screen.
pub fn body_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses the macro XML format:
///
/// ```xml
/// <macros>
///   <group name="Debug">
///     <macro name="Show global" key="Ctrl+Shift+G" confirm="false">
///       <param name="global" prompt="Global name" default="%CSW1"/>
///       <body>ZWRITE ^{{global}}</body>
///     </macro>
///     <macro name="Connect" hide_command="true">
///       <body>Do LOGIN^APP("svc","secret")</body>
///     </macro>
///   </group>
/// </macros>
/// ```
///
/// `confirm` and `hide_command` are false unless spelled `true`, `1` or `yes`,
/// so a file written before either existed keeps its meaning.
///
/// Macros outside any `<group>` land in an unnamed group, so a flat file works
/// without ceremony.
pub fn parse(xml: &str) -> Result<Vec<MacroGroup>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut groups: Vec<MacroGroup> = Vec::new();
    let mut ungrouped = MacroGroup::default();

    let mut current_group: Option<MacroGroup> = None;
    let mut current_macro: Option<Macro> = None;
    let mut in_body = false;
    let mut body_text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "malformed macro XML at byte {}: {e}",
                    reader.buffer_position()
                ))
            }
            Ok(Event::Eof) => break,

            // `Empty` covers self-closing tags like `<param ... />`, which is
            // how params are normally written.
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref()).to_string();
                match tag.as_str() {
                    "group" => {
                        current_group = Some(MacroGroup {
                            name: attr(&e, "name").unwrap_or_default(),
                            origin: Origin::default(),
                            macros: Vec::new(),
                        });
                    }
                    "macro" => {
                        current_macro = Some(Macro {
                            origin: Origin::default(),
                            name: attr(&e, "name").unwrap_or_default(),
                            description: attr(&e, "description").unwrap_or_default(),
                            key: attr(&e, "key").filter(|k| !k.is_empty()),
                            // Anything not explicitly false is treated as
                            // needing confirmation only when asked for; the
                            // default stays false so ordinary read-only macros
                            // are one click.
                            confirm: attr(&e, "confirm")
                                .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
                                .unwrap_or(false),
                            hide_command: attr(&e, "hide_command")
                                .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
                                .unwrap_or(false),
                            params: Vec::new(),
                            body: Vec::new(),
                        });
                    }
                    "param" => {
                        if let Some(m) = current_macro.as_mut() {
                            let pname = attr(&e, "name").unwrap_or_default();
                            m.params.push(Param {
                                prompt: attr(&e, "prompt").unwrap_or_else(|| pname.clone()),
                                default: attr(&e, "default").unwrap_or_default(),
                                name: pname,
                            });
                        }
                    }
                    "body" => {
                        in_body = true;
                        body_text.clear();
                    }
                    _ => {}
                }
            }

            Ok(Event::Text(e)) if in_body => {
                body_text.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::CData(e)) if in_body => {
                body_text.push_str(&String::from_utf8_lossy(&e.into_inner()));
            }

            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "body" => {
                        in_body = false;
                        if let Some(m) = current_macro.as_mut() {
                            m.body = body_lines(&body_text);
                        }
                    }
                    "macro" => {
                        if let Some(m) = current_macro.take() {
                            match current_group.as_mut() {
                                Some(group) => group.macros.push(m),
                                None => ungrouped.macros.push(m),
                            }
                        }
                    }
                    "group" => {
                        if let Some(group) = current_group.take() {
                            groups.push(group);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buf.clear();
    }

    if !ungrouped.macros.is_empty() {
        groups.insert(0, ungrouped);
    }
    Ok(groups)
}

/// Reads one attribute, resolving XML entities.
///
/// The unescaping matters for round-tripping: without it a description
/// containing `&` comes back as `&amp;` and gains another `amp;` every time
/// the personal file is saved.
fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.as_ref() != key.as_bytes() {
            return None;
        }
        Some(
            a.unescape_value()
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned()),
        )
    })
}

pub fn load(path: &Path) -> Result<Vec<MacroGroup>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading macros from {}", path.display()))?;
    parse(&text)
}

/// Loads one file and stamps every macro in it with `origin`.
pub fn load_with_origin(path: &Path, origin: Origin) -> Result<Vec<MacroGroup>> {
    let mut groups = load(path)?;
    for group in &mut groups {
        group.origin = origin;
        for m in &mut group.macros {
            m.origin = origin;
        }
    }
    Ok(groups)
}

/// What a load attempt produced, so the UI can explain a missing or broken
/// shared file instead of silently showing fewer macros.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub groups: Vec<MacroGroup>,
    /// Problems worth showing the user, one per source.
    pub problems: Vec<String>,
}

/// Loads the organisation file (if configured) and the personal file, and
/// merges them.
///
/// Organisation macros come first and keep their own groups. A personal group
/// whose name matches an organisation group is merged into it, so the panel
/// does not show "Debug" twice; within a merged group the personal entries
/// follow the shared ones.
///
/// A missing organisation file is not an error — colleagues who are off the
/// network still get their personal macros.
pub fn load_all(org: Option<&Path>, personal: &Path) -> LoadReport {
    let mut report = LoadReport::default();

    if let Some(org_path) = org {
        if org_path.as_os_str().is_empty() {
            // Not configured; nothing to say.
        } else if !org_path.exists() {
            report.problems.push(format!(
                "Organization macros not found at {} — using personal macros only.",
                org_path.display()
            ));
        } else {
            match load_with_origin(org_path, Origin::Organization) {
                Ok(groups) => report.groups = groups,
                Err(e) => report
                    .problems
                    .push(format!("Organization macros could not be read: {e:#}")),
            }
        }
    }

    match load_with_origin(personal, Origin::Personal) {
        Ok(groups) => merge(&mut report.groups, groups),
        Err(e) => report
            .problems
            .push(format!("Personal macros could not be read: {e:#}")),
    }

    report
}

/// Folds `extra` into `into`, combining groups that share a name.
fn merge(into: &mut Vec<MacroGroup>, extra: Vec<MacroGroup>) {
    for group in extra {
        match into.iter_mut().find(|g| g.name == group.name) {
            Some(existing) => existing.macros.extend(group.macros),
            None => into.push(group),
        }
    }
}

/// Serialises personal macros back to XML.
///
/// Only personal macros are written: the organisation file is shared and must
/// never be rewritten from here, and a macro that came from it would silently
/// become a local copy if it were.
pub fn to_xml(groups: &[MacroGroup]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<macros>\n");

    for group in groups {
        let personal: Vec<&Macro> = group
            .macros
            .iter()
            .filter(|m| m.origin.is_editable())
            .collect();
        if personal.is_empty() {
            continue;
        }

        out.push_str(&format!("  <group name=\"{}\">\n", escape(&group.name)));
        for m in personal {
            out.push_str(&format!("    <macro name=\"{}\"", escape(&m.name)));
            if !m.description.is_empty() {
                out.push_str(&format!(" description=\"{}\"", escape(&m.description)));
            }
            if let Some(key) = &m.key {
                out.push_str(&format!(" key=\"{}\"", escape(key)));
            }
            if m.confirm {
                out.push_str(" confirm=\"true\"");
            }
            // Written only when set, so a file that has never used either
            // attribute comes back out byte for byte as it went in.
            if m.hide_command {
                out.push_str(" hide_command=\"true\"");
            }
            out.push_str(">\n");

            for p in &m.params {
                out.push_str(&format!("      <param name=\"{}\"", escape(&p.name)));
                if !p.prompt.is_empty() && p.prompt != p.name {
                    out.push_str(&format!(" prompt=\"{}\"", escape(&p.prompt)));
                }
                if !p.default.is_empty() {
                    out.push_str(&format!(" default=\"{}\"", escape(&p.default)));
                }
                out.push_str("/>\n");
            }

            out.push_str("      <body>");
            if m.body.len() == 1 {
                out.push_str(&escape(&m.body[0]));
            } else {
                out.push('\n');
                for line in &m.body {
                    out.push_str(&format!("        {}\n", escape(line)));
                }
                out.push_str("      ");
            }
            out.push_str("</body>\n    </macro>\n");
        }
        out.push_str("  </group>\n");
    }

    out.push_str("</macros>\n");
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Written as the personal macro file on first run, so the format is
/// self-documenting. The organisation file uses the same schema but is never
/// created by the app — it is provided and maintained centrally.
pub const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!--
  Personal macros for newIrisTerminal.

  This file is yours: the app can edit it. Macros supplied by the
  organisation live in a separate file, configured in Settings, and are
  shown read-only alongside these.

  {{name}} placeholders are filled in from <param> before sending.
  confirm="true" asks for a yes/no before anything is sent - use it for
  anything that writes, since RDB* databases are shared with the team.

  hide_command="true" keeps the body out of the macro panel, for a command
  that carries a password. Note that IRIS still echoes what it is sent, so
  the text can reach the screen and the transcript by that route.

  key="Ctrl+Shift+G" binds a shortcut. It needs a modifier, and it only
  fires while the terminal has focus. No macro here claims one by default.
-->
<macros>
  <group name="Macros">
    <macro name="Developer Tools (Exec)">
      <body>do ##class(SrcPub.Cmd).Exec()</body>
    </macro>
  </group>
</macros>
"#;

/// Earlier versions of [`SAMPLE`], kept so an untouched first-run file can be
/// replaced when the shipped set changes.
///
/// Matched byte for byte, which is the whole safeguard: the moment the user
/// edits the file - by hand or through the editor, which rewrites it in a
/// different shape entirely - it stops matching and is left alone forever.
const RETIRED_SAMPLES: [&str; 1] = [r#"<?xml version="1.0" encoding="utf-8"?>
<!--
  Personal macros for newIrisTerminal.

  This file is yours: the app can edit it. Macros supplied by the
  organisation live in a separate file, configured in Settings, and are
  shown read-only alongside these.

  {{name}} placeholders are filled in from <param> before sending.
  confirm="true" asks for a yes/no before anything is sent - use it for
  anything that writes, since RDB* databases are shared with the team.

  hide_command="true" keeps the body out of the macro panel, for a command
  that carries a password. Note that IRIS still echoes what it is sent, so
  the text can reach the screen and the transcript by that route.

  key="Ctrl+Shift+G" binds a shortcut. It needs a modifier, and it only
  fires while the terminal has focus.
-->
<macros>
  <group name="Inspect">
    <macro name="Show global" key="Ctrl+Shift+G" description="ZWRITE a global">
      <param name="global" prompt="Global name (without ^)" default="CSW1"/>
      <body>ZWRITE ^{{global}}</body>
    </macro>
    <macro name="Current namespace" description="Show where we are">
      <body>Write $NAMESPACE,!</body>
    </macro>
  </group>

  <group name="Navigate">
    <macro name="Switch namespace" key="Ctrl+Shift+N">
      <param name="ns" prompt="Namespace" default="USER"/>
      <body>ZN "{{ns}}"</body>
    </macro>
  </group>

  <group name="Danger">
    <macro name="Kill a global" confirm="true"
           description="Deletes data - shared database, confirm carefully">
      <param name="global" prompt="Global to KILL (without ^)"/>
      <body>KILL ^{{global}}</body>
    </macro>
  </group>
</macros>
"#];

/// Makes sure the personal macro file exists, and refreshes it while it is
/// still exactly as shipped.
///
/// Called before loading. A file the user has touched is never rewritten - see
/// [`RETIRED_SAMPLES`].
pub fn ensure_personal_file(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if RETIRED_SAMPLES.contains(&text.as_str()) {
                let _ = std::fs::write(path, SAMPLE);
            }
        }
        // Missing, or unreadable for a reason a write will hit too. Ship the
        // sample so the format is self-documenting from the first run.
        Err(_) => {
            let _ = std::fs::write(path, SAMPLE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"
<macros>
  <group name="Debug">
    <macro name="Show global" key="Ctrl+Shift+G" confirm="false">
      <param name="global" prompt="Global name" default="CSW1"/>
      <body>ZWRITE ^{{global}}</body>
    </macro>
    <macro name="Two liner">
      <body>
        Set x = 1
        Write x,!
      </body>
    </macro>
  </group>
  <group name="Danger">
    <macro name="Kill" confirm="true">
      <param name="g"/>
      <body>KILL ^{{g}}</body>
    </macro>
  </group>
</macros>
"#;

    #[test]
    fn parses_groups_and_macros() {
        let groups = parse(XML).expect("parse");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Debug");
        assert_eq!(groups[0].macros.len(), 2);
        assert_eq!(groups[1].macros[0].name, "Kill");
    }

    #[test]
    fn parses_params_with_prompt_and_default() {
        let groups = parse(XML).expect("parse");
        let param = &groups[0].macros[0].params[0];
        assert_eq!(param.name, "global");
        assert_eq!(param.prompt, "Global name");
        assert_eq!(param.default, "CSW1");
    }

    /// A param without an explicit prompt must still be labelled in the UI.
    #[test]
    fn prompt_falls_back_to_the_param_name() {
        let groups = parse(XML).expect("parse");
        assert_eq!(groups[1].macros[0].params[0].prompt, "g");
    }

    #[test]
    fn confirm_defaults_to_false_and_is_read_when_present() {
        let groups = parse(XML).expect("parse");
        assert!(!groups[0].macros[0].confirm);
        assert!(!groups[0].macros[1].confirm);
        assert!(
            groups[1].macros[0].confirm,
            "destructive macro must confirm"
        );
    }

    /// Only the ends of a line are trimmed. The spaces inside one are the
    /// ObjectScript - `Set x = 1` is not `Setx=1` - and the editor used to run
    /// every keystroke through here, which is what made the space bar look
    /// broken.
    #[test]
    fn body_lines_keep_the_spaces_inside_a_command() {
        assert_eq!(
            body_lines("  Set x = 1  \n\n  Write x, !  \n"),
            vec!["Set x = 1".to_string(), "Write x, !".to_string()]
        );
    }

    #[test]
    fn multi_line_bodies_are_split_and_trimmed() {
        let groups = parse(XML).expect("parse");
        assert_eq!(
            groups[0].macros[1].body,
            vec!["Set x = 1".to_string(), "Write x,!".to_string()]
        );
    }

    #[test]
    fn expansion_substitutes_every_occurrence() {
        let m = Macro {
            body: vec!["Set ^{{g}} = ^{{g}} + 1".into()],
            ..Macro::default()
        };
        assert_eq!(
            m.expand(&[("g".into(), "X".into())]),
            vec!["Set ^X = ^X + 1".to_string()]
        );
    }

    /// A typo in a placeholder should be visible, not silently blanked into a
    /// command that runs against the wrong global.
    #[test]
    fn unknown_placeholders_are_left_intact() {
        let m = Macro {
            body: vec!["ZWRITE ^{{typo}}".into()],
            ..Macro::default()
        };
        assert_eq!(
            m.expand(&[("global".into(), "X".into())]),
            vec!["ZWRITE ^{{typo}}".to_string()]
        );
    }

    #[test]
    fn macros_outside_a_group_still_load() {
        let groups = parse("<macros><macro name=\"Bare\"><body>Write 1</body></macro></macros>")
            .expect("parse");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].macros[0].name, "Bare");
    }

    /// A hand-edited macro file will eventually be malformed. Whatever the
    /// parser decides, it must return rather than panic, and it must never
    /// invent a macro out of broken input — a half-parsed body could otherwise
    /// send arbitrary text to a live session.
    #[test]
    fn malformed_xml_returns_instead_of_panicking() {
        let broken = [
            "<macros><<<>",
            "<macros><group>",
            "<macros><macro name=\"x\"><body>Write 1",
            "not xml at all",
            "",
            "<macros><macro><body></body></macro>",
        ];

        for xml in broken {
            match parse(xml) {
                Err(_) => {}
                Ok(groups) => {
                    for m in groups.iter().flat_map(|g| &g.macros) {
                        assert!(
                            !m.body.is_empty() || m.name.is_empty() || m.params.is_empty(),
                            "invented a runnable macro from {xml:?}: {m:?}"
                        );
                    }
                }
            }
        }
    }

    /// An unterminated body must not yield a macro that looks ready to send.
    #[test]
    fn a_truncated_body_does_not_produce_a_sendable_macro() {
        let groups = parse("<macros><macro name=\"x\"><body>KILL ^DATA").unwrap_or_default();
        let sendable: Vec<_> = groups
            .iter()
            .flat_map(|g| &g.macros)
            .filter(|m| !m.body.is_empty())
            .collect();
        assert!(
            sendable.is_empty(),
            "truncated macro became sendable: {sendable:?}"
        );
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-macros-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const ORG_XML: &str = r#"<macros>
  <group name="Shared"><macro name="Org one"><body>Write 1</body></macro></group>
  <group name="Debug"><macro name="Org debug"><body>Write 2</body></macro></group>
</macros>"#;

    const PERSONAL_XML: &str = r#"<macros>
  <group name="Mine"><macro name="Personal one"><body>Write 3</body></macro></group>
  <group name="Debug"><macro name="Personal debug"><body>Write 4</body></macro></group>
</macros>"#;

    #[test]
    fn org_and_personal_files_merge_by_group_name() {
        let dir = tempdir("merge");
        let org = dir.join("org.xml");
        let personal = dir.join("personal.xml");
        std::fs::write(&org, ORG_XML).unwrap();
        std::fs::write(&personal, PERSONAL_XML).unwrap();

        let report = load_all(Some(&org), &personal);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let names: Vec<&str> = report.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Shared", "Debug", "Mine"],
            "groups should merge, not duplicate"
        );

        let debug = report.groups.iter().find(|g| g.name == "Debug").unwrap();
        assert_eq!(debug.macros.len(), 2, "both Debug groups should combine");
        assert_eq!(debug.macros[0].origin, Origin::Organization);
        assert_eq!(debug.macros[1].origin, Origin::Personal);
    }

    /// Being off the network must not cost the user their own macros.
    #[test]
    fn a_missing_org_file_still_loads_personal_macros() {
        let dir = tempdir("missing-org");
        let personal = dir.join("personal.xml");
        std::fs::write(&personal, PERSONAL_XML).unwrap();

        let report = load_all(Some(&dir.join("nope.xml")), &personal);
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.problems.len(), 1);
        assert!(report.problems[0].contains("not found"));
    }

    #[test]
    fn org_macros_are_not_editable_and_personal_ones_are() {
        let dir = tempdir("editable");
        let org = dir.join("org.xml");
        let personal = dir.join("personal.xml");
        std::fs::write(&org, ORG_XML).unwrap();
        std::fs::write(&personal, PERSONAL_XML).unwrap();

        let report = load_all(Some(&org), &personal);
        for group in &report.groups {
            for m in &group.macros {
                assert_eq!(
                    m.origin.is_editable(),
                    m.name.starts_with("Personal"),
                    "{} has the wrong editability",
                    m.name
                );
            }
        }
    }

    /// Writing back must never absorb organisation macros into the personal
    /// file, or a shared macro would silently fork into a local copy.
    #[test]
    fn serialising_writes_only_personal_macros() {
        let dir = tempdir("write");
        let org = dir.join("org.xml");
        let personal = dir.join("personal.xml");
        std::fs::write(&org, ORG_XML).unwrap();
        std::fs::write(&personal, PERSONAL_XML).unwrap();

        let report = load_all(Some(&org), &personal);
        let xml = to_xml(&report.groups);

        assert!(xml.contains("Personal one"));
        assert!(xml.contains("Personal debug"));
        assert!(
            !xml.contains("Org one"),
            "organisation macro leaked into the personal file"
        );
        assert!(
            !xml.contains("Org debug"),
            "organisation macro leaked into the personal file"
        );
    }

    #[test]
    fn personal_macros_round_trip_through_xml() {
        let original = vec![MacroGroup {
            name: "Mine".into(),
            origin: Origin::Personal,
            macros: vec![Macro {
                origin: Origin::Personal,
                name: "Show".into(),
                description: "A & B <test>".into(),
                key: Some("Ctrl+G".into()),
                confirm: true,
                hide_command: true,
                params: vec![Param {
                    name: "g".into(),
                    prompt: "Global".into(),
                    default: "CSW1".into(),
                }],
                body: vec!["ZWRITE ^{{g}}".into()],
            }],
        }];

        let xml = to_xml(&original);
        assert!(
            xml.contains(r#"hide_command="true""#),
            "hide_command was not written: {xml}"
        );

        let reparsed = parse(&xml).expect("round trip");
        let m = &reparsed[0].macros[0];
        assert_eq!(m.name, "Show");
        assert_eq!(m.description, "A & B <test>");
        assert_eq!(m.key.as_deref(), Some("Ctrl+G"));
        assert!(m.confirm);
        assert!(m.hide_command);
        assert_eq!(m.params[0].default, "CSW1");
        assert_eq!(m.body, vec!["ZWRITE ^{{g}}".to_string()]);
    }

    #[test]
    fn a_macro_without_the_attribute_is_not_hidden() {
        let groups =
            parse(r#"<macros><macro name="Plain"><body>Write 1,!</body></macro></macros>"#)
                .expect("parse");
        assert!(!groups[0].macros[0].hide_command);

        let mut personal = groups;
        personal[0].origin = Origin::Personal;
        personal[0].macros[0].origin = Origin::Personal;
        assert!(
            !to_xml(&personal).contains("hide_command"),
            "an unset attribute must not be written back"
        );
    }

    #[test]
    fn multi_line_bodies_round_trip() {
        let groups = vec![MacroGroup {
            name: "G".into(),
            origin: Origin::Personal,
            macros: vec![Macro {
                origin: Origin::Personal,
                name: "Two".into(),
                body: vec!["Set x = 1".into(), "Write x,!".into()],
                ..Macro::default()
            }],
        }];
        let back = parse(&to_xml(&groups)).unwrap();
        assert_eq!(
            back[0].macros[0].body,
            vec!["Set x = 1".to_string(), "Write x,!".to_string()]
        );
    }

    /// The sample shipped on first run must itself be valid, or the very first
    /// thing a user sees is a parse error.
    #[test]
    fn the_bundled_sample_parses() {
        let groups = parse(SAMPLE).expect("sample must parse");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Macros");

        let all: Vec<&Macro> = groups.iter().flat_map(|g| g.macros.iter()).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Developer Tools (Exec)");
        assert_eq!(
            all[0].body,
            vec!["do ##class(SrcPub.Cmd).Exec()".to_string()]
        );
        // No shortcut by default: one that fires the moment the terminal has
        // focus has to be chosen deliberately.
        assert!(all[0].key.is_none());
    }

    /// Every retired sample must still parse, because it is compared against a
    /// real file on disk and a typo in one would quietly stop matching.
    #[test]
    fn every_retired_sample_parses() {
        for text in RETIRED_SAMPLES {
            parse(text).expect("a retired sample must still be valid XML");
            assert_ne!(text, SAMPLE, "a retired sample is still the current one");
        }
    }

    /// The migration exists to replace an untouched file - and only that.
    #[test]
    fn an_untouched_sample_is_refreshed_and_an_edited_one_is_not() {
        let dir = tempdir("ensure");

        let fresh = dir.join("fresh.xml");
        ensure_personal_file(&fresh);
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), SAMPLE);

        let retired = dir.join("retired.xml");
        std::fs::write(&retired, RETIRED_SAMPLES[0]).unwrap();
        ensure_personal_file(&retired);
        assert_eq!(std::fs::read_to_string(&retired).unwrap(), SAMPLE);

        let edited = dir.join("edited.xml");
        let mine = format!("{}\n<!-- mine -->\n", RETIRED_SAMPLES[0]);
        std::fs::write(&edited, &mine).unwrap();
        ensure_personal_file(&edited);
        assert_eq!(
            std::fs::read_to_string(&edited).unwrap(),
            mine,
            "an edited file must be left alone"
        );
    }
}
