//! Guided access to the IRIS utilities the team actually runs.
//!
//! Each helper *composes the exact line* the user would have typed and sends
//! it, letting the output render in the terminal like any other session. That
//! keeps the terminal authoritative — no second, subtly different renderer to
//! maintain — while removing the need to remember each routine's argument
//! order.
//!
//! Everything here is a pure function from parameters to keystrokes, which is
//! what makes it testable without a live instance.

use crate::i18n::{tr1, tr2};

/// One field the helper asks for, with the value it starts out holding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    /// Label shown above the field.
    pub label: &'static str,
    /// Pre-filled value. Empty means the field starts blank.
    pub default: &'static str,
}

/// A prepared invocation: the lines to send, and what to say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub lines: Vec<String>,
    /// Shown in the status line once it has been sent. Already translated:
    /// it is read, not sent, so unlike `lines` it is not ObjectScript.
    pub summary: String,
}

/// The helpers offered in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    CompileClasses,
    GenerateInterface,
}

impl Native {
    pub const ALL: [Native; 2] = [Native::CompileClasses, Native::GenerateInterface];

    pub fn label(self) -> &'static str {
        match self {
            Native::CompileClasses => "Compile classes",
            Native::GenerateInterface => "Generate interface",
        }
    }

    /// The fields this helper needs, in the order they are asked for.
    pub fn params(self) -> &'static [Param] {
        match self {
            Native::CompileClasses => &[
                Param {
                    label: "Package",
                    default: "",
                },
                Param {
                    label: "Flag",
                    // What everyone passes: build dependencies, keep the
                    // generated source, and compile quietly.
                    default: "bkf1",
                },
            ],
            Native::GenerateInterface => &[Param {
                label: "Routine/Group",
                default: "",
            }],
        }
    }

    /// Values a freshly selected helper starts with.
    pub fn default_values(self) -> Vec<String> {
        self.params()
            .iter()
            .map(|p| p.default.to_string())
            .collect()
    }

    /// Builds the keystrokes. `values` is positional, matching [`params`];
    /// a missing entry counts as blank, so a half-filled form still previews.
    ///
    /// [`params`]: Native::params
    pub fn build(self, values: &[String]) -> Invocation {
        let at = |n: usize| -> String {
            values
                .get(n)
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        };

        match self {
            Native::CompileClasses => {
                let package = at(0);
                let flag = at(1);
                Invocation {
                    // Both arguments are strings to `CompilePackage`, so they
                    // are quoted here rather than left for the user to
                    // remember - an unquoted package name would reach IRIS as
                    // an undefined local variable.
                    lines: vec![format!(
                        "do $SYSTEM.OBJ.CompilePackage(\"{}\",\"{}\")",
                        escape_quotes(&package),
                        escape_quotes(&flag)
                    )],
                    summary: tr2("Compile classes: {} (flag {})", &package, &flag),
                }
            }

            Native::GenerateInterface => {
                let target = at(0);
                Invocation {
                    lines: vec![format!("do ^%CSW1GEN(\"{}\")", escape_quotes(&target))],
                    summary: tr1("Generate interface: {}", &target),
                }
            }
        }
    }
}

/// Keeps a stray quote from terminating the ObjectScript string early. In
/// ObjectScript a literal quote is written by doubling it.
fn escape_quotes(text: &str) -> String {
    text.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn compiling_a_package_quotes_both_arguments() {
        let inv = Native::CompileClasses.build(&values(&["Utils", "bkf1"]));
        assert_eq!(
            inv.lines,
            vec!["do $SYSTEM.OBJ.CompilePackage(\"Utils\",\"bkf1\")".to_string()]
        );
    }

    /// The flag is the one field nobody wants to retype.
    #[test]
    fn the_compile_flag_starts_at_bkf1() {
        assert_eq!(
            Native::CompileClasses.default_values(),
            vec![String::new(), "bkf1".to_string()]
        );
    }

    #[test]
    fn generating_an_interface_passes_the_routine_or_group() {
        let inv = Native::GenerateInterface.build(&values(&["CCPV005"]));
        assert_eq!(inv.lines, vec!["do ^%CSW1GEN(\"CCPV005\")".to_string()]);
    }

    #[test]
    fn arguments_are_trimmed_before_use() {
        let inv = Native::GenerateInterface.build(&values(&["  CCPV005  "]));
        assert_eq!(inv.lines, vec!["do ^%CSW1GEN(\"CCPV005\")".to_string()]);
    }

    /// A quote in a value must not break out of the string literal.
    #[test]
    fn embedded_quotes_are_doubled() {
        let inv = Native::GenerateInterface.build(&values(&["a\"b"]));
        assert_eq!(inv.lines, vec!["do ^%CSW1GEN(\"a\"\"b\")".to_string()]);
    }

    /// The panel previews the line as it is typed, so a form with fewer values
    /// than parameters must still build rather than panic.
    #[test]
    fn a_half_filled_form_still_builds() {
        let inv = Native::CompileClasses.build(&values(&["Utils"]));
        assert_eq!(
            inv.lines,
            vec!["do $SYSTEM.OBJ.CompilePackage(\"Utils\",\"\")".to_string()]
        );
        assert_eq!(Native::CompileClasses.build(&[]).lines.len(), 1);
    }

    #[test]
    fn every_native_has_a_label_and_a_field_for_each_default() {
        for native in Native::ALL {
            assert!(!native.label().is_empty());
            assert_eq!(native.default_values().len(), native.params().len());
            for param in native.params() {
                assert!(
                    !param.label.is_empty(),
                    "{native:?} has an unlabelled field"
                );
            }
        }
    }
}
