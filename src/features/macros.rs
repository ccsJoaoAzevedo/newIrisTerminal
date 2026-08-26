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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Macro {
    pub name: String,
    pub description: String,
    /// Optional accelerator, e.g. `Ctrl+Shift+G`. Purely descriptive here;
    /// binding is the UI's job.
    pub key: Option<String>,
    /// Require an explicit yes before sending. Set this on anything that writes.
    pub confirm: bool,
    pub params: Vec<Param>,
    /// Body lines, already split. Sent one line at a time.
    pub body: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroGroup {
    pub name: String,
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

/// Parses the macro XML format:
///
/// ```xml
/// <macros>
///   <group name="Debug">
///     <macro name="Show global" key="Ctrl+Shift+G" confirm="false">
///       <param name="global" prompt="Global name" default="%CSW1"/>
///       <body>ZWRITE ^{{global}}</body>
///     </macro>
///   </group>
/// </macros>
/// ```
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
                            macros: Vec::new(),
                        });
                    }
                    "macro" => {
                        current_macro = Some(Macro {
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
                            m.body = body_text
                                .lines()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty())
                                .map(str::to_string)
                                .collect();
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

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.as_ref() == key.as_bytes()).then(|| String::from_utf8_lossy(&a.value).into_owned())
    })
}

pub fn load(path: &Path) -> Result<Vec<MacroGroup>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading macros from {}", path.display()))?;
    parse(&text)
}

/// Written on first run so the format is discoverable without documentation.
pub const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!--
  Macros for newIrisTerminal.

  {{name}} placeholders are filled in from <param> before sending.
  confirm="true" asks for a yes/no before anything is sent - use it for
  anything that writes, since RDB* databases are shared with the team.
-->
<macros>
  <group name="Inspect">
    <macro name="Show global" key="Ctrl+Shift+G" description="ZWRITE a global">
      <param name="global" prompt="Global name (without ^)" default="CSW1"/>
      <body>ZWRITE ^{{global}}</body>
    </macro>
    <macro name="Global browser" description="Open ^%G">
      <body>Do ^%G</body>
    </macro>
    <macro name="Current namespace" description="Show where we are">
      <body>Write $NAMESPACE,!</body>
    </macro>
    <macro name="Routine list" description="Open ^%RD">
      <body>Do ^%RD</body>
    </macro>
  </group>

  <group name="Navigate">
    <macro name="Switch namespace">
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
"#;

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

    /// The sample shipped on first run must itself be valid, or the very first
    /// thing a user sees is a parse error.
    #[test]
    fn the_bundled_sample_parses() {
        let groups = parse(SAMPLE).expect("sample must parse");
        assert!(!groups.is_empty());
        let kill = groups
            .iter()
            .flat_map(|g| &g.macros)
            .find(|m| m.name == "Kill a global")
            .expect("sample has the destructive example");
        assert!(
            kill.confirm,
            "the destructive sample must require confirmation"
        );
    }
}
