//! Shell plugins: the other command interpreters this machine has.
//!
//! A shell plugin is a declaration, not code — a name and a program to run —
//! which is what lets it be a plugin at all. The WebAssembly plugins next door
//! are sandboxed precisely so that they *cannot* start a process; a plugin that
//! opens PowerShell therefore cannot be one of those, and does not need to be.
//! There is nothing to execute in the host: the app already knows how to give a
//! program a pseudo-terminal, because that is what it does with IRIS.
//!
//! **The folder is the list.** Every shell the app offers is a `.toml` in
//! `<config>/plugins/shells/`, with no exceptions and nothing offered from a
//! table compiled into the binary. What the platform probes do is *write those
//! files*: the first time the app runs, the shells it finds installed are
//! materialised into the folder, each one a file that can then be renamed,
//! re-armed with different arguments, or deleted like any other.
//!
//! Deleting one makes it stay deleted. A stamp file records which programs have
//! ever been written, so a probe never puts back a file the user removed - and
//! a shell installed *after* that first run is still picked up, because its
//! program is not in the stamp yet.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One shell the app can open a tab on, as its file declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    /// What the menu calls it.
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The file it came from.
    pub file: PathBuf,
    /// The app wrote this file, having found the program installed. Purely
    /// informational - it is an ordinary file either way - but it is worth
    /// saying in the settings list which entries the user chose and which ones
    /// simply appeared.
    pub generated: bool,
}

impl Shell {
    /// The command, for a tooltip and for the settings list.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            return self.program.display().to_string();
        }
        format!("{} {}", self.program.display(), self.args.join(" "))
    }

    /// How the settings list describes where it came from.
    pub fn source_label(&self) -> &'static str {
        if self.generated {
            "found on this machine"
        } else {
            "declared"
        }
    }
}

/// A shell file, as it is spelled.
///
/// ```toml
/// name = "PowerShell 7"
/// program = "C:/Program Files/PowerShell/7/pwsh.exe"
/// args = ["-NoLogo"]
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    name: String,
    program: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    /// Written by the app when it materialised a shell it found. A file the
    /// user wrote leaves it out, and leaving it out means "mine".
    #[serde(default)]
    generated: bool,
}

/// Where shell files live.
pub fn shells_dir() -> PathBuf {
    crate::config::plugins_dir().join("shells")
}

/// Records which programs have already been written out, so that deleting a
/// generated file is a decision the app respects rather than one it undoes on
/// the next start.
fn stamp_path(dir: &Path) -> PathBuf {
    dir.join(".discovered")
}

/// The header on a file the app wrote, so it is obvious which is which when the
/// folder is opened.
const GENERATED_HEADER: &str = "# Written by newIrisTerminal: this shell was found installed on this machine.\n\
                                # It is an ordinary shell plugin - rename it, change its arguments, or delete\n\
                                # it, and the app will leave your version alone.\n";

/// Probing and reading the directory costs little, but the new-session menu
/// asks on every frame it is open. Cached for the process, and dropped by
/// [`refresh`] when the user has just changed a file.
static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<Vec<Shell>>>> =
    std::sync::OnceLock::new();

fn cache() -> &'static std::sync::Mutex<Option<Vec<Shell>>> {
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Every shell the folder declares, having first written out any newly found
/// ones.
pub fn available() -> Vec<Shell> {
    if let Ok(guard) = cache().lock() {
        if let Some(found) = guard.as_ref() {
            return found.clone();
        }
    }
    let found = collect(&shells_dir());
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(found.clone());
    }
    found
}

/// Drops the cache, so the next call re-reads the folder and probes again.
pub fn refresh() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

/// Materialises what is installed, then reads the folder.
///
/// Separate from [`available`] so a test can point it at a directory of its
/// own: the probes depend on the machine, but which files get written - and
/// which do not - is the half with the decisions in it.
fn collect(dir: &Path) -> Vec<Shell> {
    materialise(dir, &discover());
    declared(dir)
}

/// Writes a file for each of `found` that has not been written before.
///
/// Three reasons to skip one, and they are the whole of the behaviour: the
/// stamp already names the program (so a file for it was written once, and
/// whether it is still there is the user's business), a file in the folder
/// already names it (so the user is managing that program by hand), or the
/// directory cannot be written at all.
fn materialise(dir: &Path, found: &[Candidate]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let stamped = read_stamp(dir);
    let existing: Vec<PathBuf> = declared(dir).into_iter().map(|s| s.program).collect();

    let mut added: Vec<String> = Vec::new();
    for candidate in found {
        let known = stamped
            .iter()
            .chain(existing.iter())
            .any(|program| same_program(program, &candidate.program));
        if known {
            continue;
        }
        let path = free_path(dir, &candidate.name);
        let manifest = Manifest {
            name: candidate.name.clone(),
            program: candidate.program.clone(),
            args: candidate.args.clone(),
            generated: true,
        };
        let Ok(body) = toml::to_string_pretty(&manifest) else {
            continue;
        };
        if std::fs::write(&path, format!("{GENERATED_HEADER}\n{body}")).is_err() {
            continue;
        }
        // Stamped only once the file is on disk, so a failed write is retried
        // on the next start rather than remembered as done.
        added.push(candidate.program.display().to_string());
    }

    if added.is_empty() {
        return;
    }
    let mut stamp: Vec<String> = stamped
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>();
    stamp.extend(added);
    let _ = std::fs::write(
        stamp_path(dir),
        format!(
            "# Programs newIrisTerminal has written a shell file for. Delete a line to have\n\
             # its file written again on the next start.\n{}\n",
            stamp.join("\n")
        ),
    );
}

/// The programs already written out, from the stamp file.
fn read_stamp(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(stamp_path(dir))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

/// A file name in `dir` that nothing is using yet, based on `name`.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let slug: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "shell".to_string()
    } else {
        slug
    };
    let first = dir.join(format!("{slug}.toml"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{slug}-{n}.toml")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Whether two paths name the same program, as far as this platform cares.
fn same_program(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        // Windows paths are case-insensitive, and a file written by hand with
        // forward slashes has to match a probe written with backslashes.
        let normalise = |p: &Path| p.to_string_lossy().to_lowercase().replace('\\', "/");
        normalise(a) == normalise(b)
    } else {
        a == b
    }
}

/// The `.toml` files in `dir`, in name order so the menu is stable.
///
/// A file that does not parse, or that names a program that is not there, is
/// skipped with a line in the log: a menu entry that cannot start anything is
/// worse than no entry.
fn declared(dir: &Path) -> Vec<Shell> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();

    let mut shells = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let manifest: Manifest = match toml::from_str(&text) {
            Ok(manifest) => manifest,
            Err(e) => {
                // A file of nothing but comments is somebody's notes, not a
                // broken plugin.
                if text.lines().any(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with('#')
                }) {
                    log::warn!("{} is not a shell plugin ({e})", path.display());
                }
                continue;
            }
        };
        if manifest.name.trim().is_empty() || manifest.program.as_os_str().is_empty() {
            continue;
        }
        if !manifest.program.is_file() {
            log::warn!(
                "shell plugin {} names a program that is not there: {}",
                path.display(),
                manifest.program.display()
            );
            continue;
        }
        shells.push(Shell {
            name: manifest.name.trim().to_string(),
            program: manifest.program,
            args: manifest.args,
            file: path,
            generated: manifest.generated,
        });
    }
    shells
}

/// A shell the platform probes found installed, before it has a file.
struct Candidate {
    name: String,
    program: PathBuf,
    args: Vec<String>,
}

/// The shells found on this machine.
fn discover() -> Vec<Candidate> {
    probes()
        .into_iter()
        .filter_map(|probe| {
            let program = probe.paths.into_iter().find(|path| path.is_file())?;
            Some(Candidate {
                name: probe.name.to_string(),
                program,
                args: probe.args.iter().map(|a| a.to_string()).collect(),
            })
        })
        .collect()
}

/// A probe: what to call it, where to look, and how to start it.
struct Probe {
    name: &'static str,
    /// Tried in order; the first that is a file wins.
    paths: Vec<PathBuf>,
    args: Vec<&'static str>,
}

#[cfg(windows)]
fn probes() -> Vec<Probe> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let system32 = PathBuf::from(&system_root).join("System32");
    let program_files = |var: &str, fallback: &str| {
        PathBuf::from(std::env::var(var).unwrap_or_else(|_| fallback.to_string()))
    };
    let pf = program_files("ProgramFiles", r"C:\Program Files");
    let pf86 = program_files("ProgramFiles(x86)", r"C:\Program Files (x86)");

    vec![
        Probe {
            name: "Command Prompt",
            paths: vec![system32.join("cmd.exe")],
            args: Vec::new(),
        },
        Probe {
            name: "Windows PowerShell",
            paths: vec![system32.join(r"WindowsPowerShell\v1.0\powershell.exe")],
            // Without it the banner is the first two lines of every session.
            args: vec!["-NoLogo"],
        },
        Probe {
            name: "PowerShell 7",
            paths: vec![
                pf.join(r"PowerShell\7\pwsh.exe"),
                pf.join(r"PowerShell\6\pwsh.exe"),
                pf86.join(r"PowerShell\7\pwsh.exe"),
            ],
            args: vec!["-NoLogo"],
        },
        Probe {
            name: "Git Bash",
            paths: vec![pf.join(r"Git\bin\bash.exe"), pf86.join(r"Git\bin\bash.exe")],
            // A login shell, which is what reads the Git profile scripts and
            // puts the Git tools on PATH; without it this is a bare bash with
            // none of what makes it Git Bash.
            args: vec!["--login", "-i"],
        },
        Probe {
            name: "WSL",
            paths: vec![system32.join("wsl.exe")],
            args: Vec::new(),
        },
    ]
}

#[cfg(not(windows))]
fn probes() -> Vec<Probe> {
    // `/etc/shells` is the machine's own answer to this question, so it is
    // preferred over a list of guesses; the guesses are only there for a system
    // that does not keep the file.
    let listed: Vec<PathBuf> = std::fs::read_to_string("/etc/shells")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/'))
        .map(PathBuf::from)
        .collect();

    let mut probes: Vec<Probe> = Vec::new();
    for (name, path) in [
        ("Bash", "/bin/bash"),
        ("Zsh", "/bin/zsh"),
        ("Fish", "/usr/bin/fish"),
        ("Sh", "/bin/sh"),
    ] {
        let candidate = PathBuf::from(path);
        // Only what the machine lists, when it lists anything at all: a shell
        // present but not in `/etc/shells` is one the system does not offer as
        // a login shell, and offering it here would be second-guessing that.
        if !listed.is_empty() && !listed.iter().any(|listed| listed == &candidate) {
            continue;
        }
        probes.push(Probe {
            name,
            paths: vec![candidate],
            args: Vec::new(),
        });
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-shells-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory to write into");
        dir
    }

    fn candidate(name: &str, program: &Path) -> Candidate {
        Candidate {
            name: name.to_string(),
            program: program.to_path_buf(),
            args: vec!["--login".to_string()],
        }
    }

    /// This test's own executable: the one program certain to be on disk.
    fn a_real_program() -> PathBuf {
        std::env::current_exe().expect("our own path")
    }

    /// The whole point of the folder being the list: a shell found installed
    /// arrives as a file, with its name and arguments in it, that the user can
    /// then edit like any other.
    #[test]
    fn a_found_shell_is_written_into_the_folder() {
        let dir = temp_dir("materialise");
        let program = a_real_program();
        materialise(&dir, &[candidate("Test Shell", &program)]);

        let shells = declared(&dir);
        assert_eq!(shells.len(), 1, "one file, one shell");
        assert_eq!(shells[0].name, "Test Shell");
        assert_eq!(shells[0].program, program);
        assert_eq!(shells[0].args, vec!["--login".to_string()]);
        assert!(shells[0].generated, "the app wrote this one");
        assert_eq!(
            shells[0].file,
            dir.join("test-shell.toml"),
            "named after the shell, so the folder reads as a list of shells"
        );
    }

    /// Deleting a generated file has to stick. Without the stamp, every start
    /// would put back the shell the user had just thrown away.
    #[test]
    fn a_deleted_file_is_not_written_again() {
        let dir = temp_dir("deleted");
        let program = a_real_program();
        let found = [candidate("Test Shell", &program)];

        materialise(&dir, &found);
        let file = dir.join("test-shell.toml");
        assert!(file.is_file());
        std::fs::remove_file(&file).expect("delete it");

        materialise(&dir, &found);
        assert!(!file.exists(), "it must stay deleted");
        assert!(declared(&dir).is_empty());
    }

    /// And an edited one is left exactly as edited, however many times the app
    /// starts.
    #[test]
    fn an_edited_file_is_left_alone() {
        let dir = temp_dir("edited");
        let program = a_real_program();
        let found = [candidate("Test Shell", &program)];

        materialise(&dir, &found);
        let file = dir.join("test-shell.toml");
        let mine = format!(
            "name = \"Mine\"\nprogram = {}\n",
            toml::Value::String(program.display().to_string())
        );
        std::fs::write(&file, &mine).expect("write");

        materialise(&dir, &found);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), mine);
        let shells = declared(&dir);
        assert_eq!(shells.len(), 1, "and no second file beside it");
        assert_eq!(shells[0].name, "Mine");
        assert!(!shells[0].generated, "an edited file is the user's");
    }

    /// A shell the user declared by hand, for a program the probes also find,
    /// is not doubled up: the file already there is the answer for that
    /// program.
    #[test]
    fn a_hand_written_file_stops_the_same_program_being_written() {
        let dir = temp_dir("byhand");
        let program = a_real_program();
        std::fs::write(
            dir.join("mine.toml"),
            format!(
                "name = \"Mine\"\nprogram = {}\n",
                toml::Value::String(program.display().to_string())
            ),
        )
        .expect("write");

        materialise(&dir, &[candidate("Test Shell", &program)]);
        let shells = declared(&dir);
        assert_eq!(shells.len(), 1);
        assert_eq!(shells[0].name, "Mine");
    }

    /// A shell installed after the first run still turns up: the stamp names
    /// programs, not "we have looked once".
    #[test]
    fn a_shell_installed_later_is_still_written() {
        let dir = temp_dir("later");
        let program = a_real_program();
        materialise(&dir, &[candidate("First", &program)]);
        assert_eq!(declared(&dir).len(), 1);

        // A second program appears. Any real path will do, and the parent
        // directory of our own executable is certain to be one.
        let second = program.parent().expect("a parent").join(
            program
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .to_string(),
        );
        let mut found = vec![candidate("First", &program)];
        found.push(Candidate {
            name: "Second".into(),
            program: second,
            args: Vec::new(),
        });
        materialise(&dir, &found);
        // Same program under a different spelling of the path, so it is still
        // one shell - which is the case `same_program` is there for.
        assert_eq!(declared(&dir).len(), 1);
    }

    /// Two shells with the same name must not fight over one file name.
    #[test]
    fn two_shells_with_one_name_get_two_files() {
        let dir = temp_dir("names");
        assert_eq!(free_path(&dir, "Git Bash"), dir.join("git-bash.toml"));
        std::fs::write(dir.join("git-bash.toml"), "").expect("write");
        assert_eq!(free_path(&dir, "Git Bash"), dir.join("git-bash-2.toml"));
        // A name with nothing usable in it still gets a file.
        assert_eq!(free_path(&dir, "***"), dir.join("shell.toml"));
    }

    /// A file naming a program that is not on this machine is dropped: a menu
    /// entry that cannot start anything is worse than no entry.
    #[test]
    fn a_file_pointing_at_nothing_is_dropped() {
        let dir = temp_dir("missing");
        std::fs::write(
            dir.join("ghost.toml"),
            "name = \"Ghost\"\nprogram = \"/nowhere/at/all/ghost\"\n",
        )
        .expect("write");
        assert!(declared(&dir).is_empty());
    }

    /// Whatever this machine has, every probe hit has to name a program that
    /// is really there and a name to show.
    #[test]
    fn every_discovered_shell_exists_and_is_named() {
        for candidate in discover() {
            assert!(!candidate.name.is_empty());
            assert!(
                candidate.program.is_file(),
                "{} was offered but is not there",
                candidate.program.display()
            );
        }
    }

    /// Windows paths differ in case and in slash direction between a probe and
    /// a hand-written file, and both name the same program.
    #[test]
    fn the_same_program_is_recognised_however_it_is_spelled() {
        assert!(same_program(
            Path::new("C:/Windows/System32/cmd.exe"),
            Path::new("C:/Windows/System32/cmd.exe")
        ));
        if cfg!(windows) {
            assert!(same_program(
                Path::new(r"C:\Windows\System32\cmd.exe"),
                Path::new("c:/windows/system32/CMD.EXE")
            ));
        }
        assert!(!same_program(Path::new("/bin/bash"), Path::new("/bin/zsh")));
    }
}
