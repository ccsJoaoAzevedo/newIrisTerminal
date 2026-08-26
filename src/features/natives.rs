//! Guided access to the IRIS native utilities.
//!
//! These routines are interactive and prompt-driven, so the honest way to help
//! is to *compose the exact input* the user would have typed and send it,
//! letting the output render in the terminal like any other session. That keeps
//! the terminal authoritative — no second, subtly different renderer to
//! maintain — while removing the need to remember each routine's argument
//! order.
//!
//! Everything here is a pure function from parameters to keystrokes, which is
//! what makes it testable without a live instance.

/// A prepared invocation: the lines to send, and whether it needs a yes/no
/// first because it can modify data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub lines: Vec<String>,
    pub confirm: bool,
    /// Shown in the confirmation dialog and the status line.
    pub summary: String,
}

impl Invocation {
    fn read_only(summary: impl Into<String>, lines: Vec<String>) -> Self {
        Invocation {
            lines,
            confirm: false,
            summary: summary.into(),
        }
    }
}

/// The helpers offered in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    GlobalBrowser,
    RoutineDisplay,
    RoutineSearch,
    ZwriteGlobal,
    NamespaceSwitch,
    ShowNamespace,
}

impl Native {
    pub const ALL: [Native; 6] = [
        Native::GlobalBrowser,
        Native::ZwriteGlobal,
        Native::RoutineDisplay,
        Native::RoutineSearch,
        Native::NamespaceSwitch,
        Native::ShowNamespace,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Native::GlobalBrowser => "Global browser (^%G)",
            Native::RoutineDisplay => "Routine display (^%RD)",
            Native::RoutineSearch => "Routine search (^%RS)",
            Native::ZwriteGlobal => "Inspect global (ZWRITE)",
            Native::NamespaceSwitch => "Switch namespace (ZN)",
            Native::ShowNamespace => "Show current namespace",
        }
    }

    /// The single free-text argument this helper needs, if any.
    pub fn argument_prompt(self) -> Option<&'static str> {
        match self {
            Native::GlobalBrowser => Some("Global name (without ^), blank for the menu"),
            Native::RoutineDisplay | Native::RoutineSearch => Some("Routine name or mask"),
            Native::ZwriteGlobal => Some("Global name (without ^)"),
            Native::NamespaceSwitch => Some("Namespace"),
            Native::ShowNamespace => None,
        }
    }

    /// Builds the keystrokes. `argument` is whatever the user typed, already
    /// trimmed; an empty value means "no argument given".
    pub fn build(self, argument: &str) -> Invocation {
        let arg = argument.trim();

        match self {
            Native::ShowNamespace => Invocation::read_only(
                "Show current namespace",
                vec!["Write $NAMESPACE,!".to_string()],
            ),

            Native::NamespaceSwitch => Invocation::read_only(
                format!("Switch to namespace {arg}"),
                // Quoted, because namespace names may contain % and are
                // case-sensitive to ZN.
                vec![format!("ZN \"{}\"", escape_quotes(arg))],
            ),

            Native::ZwriteGlobal => Invocation::read_only(
                format!("ZWRITE ^{arg}"),
                vec![format!("ZWRITE ^{}", strip_caret(arg))],
            ),

            Native::GlobalBrowser => {
                // ^%G prompts for the global name; sending it on the following
                // line answers that prompt. With no argument, just open it.
                let mut lines = vec!["Do ^%G".to_string()];
                if !arg.is_empty() {
                    lines.push(format!("^{}", strip_caret(arg)));
                }
                Invocation::read_only("Global browser (^%G)", lines)
            }

            Native::RoutineDisplay => {
                let mut lines = vec!["Do ^%RD".to_string()];
                if !arg.is_empty() {
                    lines.push(arg.to_string());
                }
                Invocation::read_only("Routine display (^%RD)", lines)
            }

            Native::RoutineSearch => {
                let mut lines = vec!["Do ^%RS".to_string()];
                if !arg.is_empty() {
                    lines.push(arg.to_string());
                }
                Invocation::read_only("Routine search (^%RS)", lines)
            }
        }
    }
}

/// Users habitually type the leading `^`; accept it either way.
fn strip_caret(name: &str) -> &str {
    name.trim().trim_start_matches('^')
}

/// Keeps a stray quote from terminating the ObjectScript string early. In
/// ObjectScript a literal quote is written by doubling it.
fn escape_quotes(text: &str) -> String {
    text.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_namespace_needs_no_argument() {
        assert!(Native::ShowNamespace.argument_prompt().is_none());
        let inv = Native::ShowNamespace.build("");
        assert_eq!(inv.lines, vec!["Write $NAMESPACE,!".to_string()]);
    }

    #[test]
    fn namespace_switch_quotes_the_name() {
        let inv = Native::NamespaceSwitch.build("%SYS");
        assert_eq!(inv.lines, vec!["ZN \"%SYS\"".to_string()]);
    }

    /// A quote in the argument must not break out of the string literal.
    #[test]
    fn namespace_switch_escapes_embedded_quotes() {
        let inv = Native::NamespaceSwitch.build("a\"b");
        assert_eq!(inv.lines, vec!["ZN \"a\"\"b\"".to_string()]);
    }

    #[test]
    fn zwrite_accepts_the_global_with_or_without_a_caret() {
        assert_eq!(
            Native::ZwriteGlobal.build("CSW1").lines,
            Native::ZwriteGlobal.build("^CSW1").lines
        );
        assert_eq!(
            Native::ZwriteGlobal.build("^CSW1").lines,
            vec!["ZWRITE ^CSW1".to_string()]
        );
    }

    #[test]
    fn the_global_browser_opens_bare_when_no_global_is_given() {
        assert_eq!(
            Native::GlobalBrowser.build("").lines,
            vec!["Do ^%G".to_string()]
        );
    }

    #[test]
    fn the_global_browser_answers_its_prompt_when_a_global_is_given() {
        assert_eq!(
            Native::GlobalBrowser.build("CSW1").lines,
            vec!["Do ^%G".to_string(), "^CSW1".to_string()]
        );
    }

    #[test]
    fn routine_helpers_pass_the_mask_through() {
        assert_eq!(
            Native::RoutineDisplay.build("CCPV*").lines,
            vec!["Do ^%RD".to_string(), "CCPV*".to_string()]
        );
        assert_eq!(
            Native::RoutineSearch.build("CCPV*").lines,
            vec!["Do ^%RS".to_string(), "CCPV*".to_string()]
        );
    }

    #[test]
    fn arguments_are_trimmed_before_use() {
        assert_eq!(
            Native::ZwriteGlobal.build("  CSW1  ").lines,
            vec!["ZWRITE ^CSW1".to_string()]
        );
    }

    /// None of these helpers write, so none should demand confirmation — the
    /// confirmation budget belongs to macros that actually modify data.
    #[test]
    fn every_native_helper_is_read_only() {
        for native in Native::ALL {
            assert!(
                !native.build("X").confirm,
                "{native:?} unexpectedly requires confirmation"
            );
        }
    }

    #[test]
    fn every_native_has_a_label() {
        for native in Native::ALL {
            assert!(!native.label().is_empty());
        }
    }
}
