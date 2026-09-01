//! The server list the InterSystems launcher keeps.
//!
//! The tray icon's "Servidor Preferencial" submenu and its Server Manager
//! ("Gerenciador de Servidores") dialog are two views of one list of named
//! connections: an address, the superserver port, and the Telnet port a
//! terminal reaches the instance on. Reading that list rather than asking the
//! user to retype it is what lets this app offer the same servers the rest of
//! the toolchain already knows about.
//!
//! On Windows the list lives in the registry, under the *legacy* `Cache` key
//! even on an IRIS-only machine — the launcher never renamed it. Both registry
//! views and both product names are tried, because which one holds the list
//! depends on the installer that wrote it.
//!
//! Nothing here is authoritative about *how* to connect: a server whose address
//! is this machine is better reached by starting a local session than by
//! logging in over Telnet. That decision belongs to
//! [`Server::target`], and the reasoning is there.
//!
//! Everywhere other than Windows there is no launcher and no such list, so
//! discovery yields nothing and the app behaves exactly as it did before.

/// One named connection from the Server Manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    /// Host name or IP. `localhost` for an instance on this machine.
    pub address: String,
    /// Superserver port, 1972 by default. Not used to open a terminal — kept
    /// because it is how the user recognises the entry in the launcher's own
    /// dialog, and it is shown in the tooltip.
    pub port: u16,
    /// Port the instance's Telnet service listens on, 23 by default. This is
    /// the one a terminal connects to.
    pub telnet: u16,
    /// Free-text note from the Server Manager, shown as a tooltip when set.
    pub comment: String,
}

/// How a session on a server should actually be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Start a session on a local instance, the way the app always has.
    Local { instance: String },
    /// Log in over Telnet, the way the launcher's own Terminal reaches a
    /// remote server.
    Telnet { address: String, port: u16 },
}

impl Server {
    /// Whether this entry points at the machine the app is running on.
    pub fn is_local(&self) -> bool {
        let address = self.address.trim();
        address.is_empty()
            || address.eq_ignore_ascii_case("localhost")
            || address == "127.0.0.1"
            || address == "::1"
            || address.eq_ignore_ascii_case("[::1]")
    }

    /// How to open a terminal on this server.
    ///
    /// A local address resolves to a local session when one of the instances on
    /// this machine answers to the same name. That is not merely faster: a local
    /// session is already authenticated, so it opens straight at a prompt, while
    /// Telnet would ask for the username and password the user has never had to
    /// type here. The name has to match a real instance for that to be safe —
    /// a server called `TESTES` pointing at `localhost` is still a Telnet
    /// login, because there is no `TESTES` instance to start.
    ///
    /// `instances` is the list discovered by [`crate::pty::launcher`].
    pub fn target(&self, instances: &[String]) -> Target {
        if self.is_local() {
            if let Some(instance) = instances
                .iter()
                .find(|i| i.eq_ignore_ascii_case(&self.name))
            {
                return Target::Local {
                    instance: instance.clone(),
                };
            }
        }
        Target::Telnet {
            // An empty address only ever means "here"; sending that to the
            // resolver would fail rather than loop back.
            address: if self.address.trim().is_empty() {
                "127.0.0.1".to_string()
            } else {
                self.address.clone()
            },
            port: self.telnet,
        }
    }

    /// An entry standing for an instance on this machine that the launcher's
    /// list does not mention.
    ///
    /// Lets a discovered instance and a configured server reach the same code,
    /// so there is one definition of what opening either one means.
    pub fn for_instance(name: &str) -> Server {
        Server {
            name: name.to_string(),
            address: "localhost".to_string(),
            port: 1972,
            telnet: 23,
            comment: String::new(),
        }
    }

    /// How the entry reads in a menu: the address, unless it says nothing the
    /// name has not already said.
    pub fn menu_label(&self) -> String {
        if self.is_local() {
            self.name.clone()
        } else {
            format!("{}  ({})", self.name, self.address)
        }
    }
}

/// The launcher's servers, and which one it treats as preferred.
#[derive(Clone, Debug, Default)]
pub struct ServerList {
    pub servers: Vec<Server>,
    /// Name of the preferred server, when the launcher names one. Not
    /// guaranteed to appear in `servers`: a server can be removed without the
    /// preference being cleared.
    pub preferred: Option<String>,
}

impl ServerList {
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Server> {
        self.servers
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The preferred server, if it is named *and* still defined.
    ///
    /// Falls back to the only server there is: a machine with one entry has no
    /// meaningful choice, and treating a missing preference as "no server"
    /// would leave the app guessing when the answer is obvious.
    pub fn preferred(&self) -> Option<&Server> {
        if let Some(server) = self.preferred.as_deref().and_then(|name| self.get(name)) {
            return Some(server);
        }
        match self.servers.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// Whether one of these servers already opens the local instance `name`.
    ///
    /// A local server entry and a discovered instance of the same name start
    /// exactly the same session, so listing both puts the same thing in the
    /// menu twice.
    pub fn covers_instance(&self, instances: &[String], name: &str) -> bool {
        self.servers
            .iter()
            .any(|server| match server.target(instances) {
                Target::Local { instance } => instance.eq_ignore_ascii_case(name),
                Target::Telnet { .. } => false,
            })
    }

    /// Every server other than `name`, in list order — what the "+" menu offers
    /// once the preferred one is already what the button itself does.
    pub fn others<'a>(&'a self, name: Option<&'a str>) -> impl Iterator<Item = &'a Server> {
        self.servers
            .iter()
            .filter(move |s| !name.is_some_and(|n| s.name.eq_ignore_ascii_case(n)))
    }
}

/// The target the launcher's tray menu is set to open, as a [`Target`].
///
/// Three cases, because the tray's submenu has three kinds of entry: one of the
/// configured servers, the local instance ("Este Servidor"), or nothing
/// resolvable. The middle one is why this is not just
/// [`ServerList::preferred`] — that entry names an *instance*, which is not in
/// the server list at all, and discarding it would silently fall back to
/// guessing.
pub fn preferred_target(list: &ServerList, instances: &[String]) -> Option<Target> {
    if let Some(server) = list.preferred() {
        return Some(server.target(instances));
    }
    // "Este Servidor": the preference names an installed instance rather than a
    // server entry.
    let named = list.preferred.as_deref()?.trim();
    let instance = instances.iter().find(|i| i.eq_ignore_ascii_case(named))?;
    Some(Target::Local {
        instance: instance.clone(),
    })
}

/// Reads the launcher's server list. Never fails: a machine with no launcher,
/// or a registry we cannot read, simply has no servers to offer.
pub fn discover() -> ServerList {
    #[cfg(windows)]
    {
        windows::read()
    }
    #[cfg(not(windows))]
    {
        // The launcher, its Server Manager and the registry it writes to are
        // Windows-only. Elsewhere the app keeps discovering local instances and
        // nothing else changes.
        ServerList::default()
    }
}

#[cfg(windows)]
mod windows {
    use super::{Server, ServerList};
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    /// Where the list has been kept, most likely first.
    ///
    /// `Cache` before `IRIS` because the launcher still writes the legacy key
    /// even on an IRIS-only install, and `WOW6432Node` before the native view
    /// because the launcher is a 32-bit program. All four are tried: which one
    /// holds the list depends on the installer that wrote it, and reading an
    /// absent key costs nothing.
    const SERVER_KEYS: [&str; 4] = [
        r"SOFTWARE\WOW6432Node\InterSystems\Cache\Servers",
        r"SOFTWARE\WOW6432Node\InterSystems\IRIS\Servers",
        r"SOFTWARE\InterSystems\Cache\Servers",
        r"SOFTWARE\InterSystems\IRIS\Servers",
    ];

    /// Every value under these keys is a string, ports included, so a port is
    /// parsed rather than read as a DWORD.
    fn string_value(key: &RegKey, name: &str) -> String {
        key.get_value::<String, _>(name).unwrap_or_default()
    }

    fn port_value(key: &RegKey, name: &str, default: u16) -> u16 {
        string_value(key, name)
            .trim()
            .parse()
            .ok()
            .filter(|p| *p != 0)
            .unwrap_or(default)
    }

    /// Product names the launcher has shipped its configuration under, newest
    /// first.
    const PRODUCTS: [&str; 2] = ["IRIS", "Cache"];

    /// The server the tray icon's "Servidor Preferencial" submenu has ticked.
    ///
    /// Kept per *installed instance*, because there is one tray icon per
    /// instance, at
    /// `{prefix}\InterSystems\{product}\Configurations\{instance}\Manager\PreferredServer`
    /// — a key whose unnamed default value holds the server name.
    ///
    /// Two neighbouring values look like this one and are not:
    ///
    /// * `Cache\Servers\Last Server Name` is Studio's *last connection*. It
    ///   happened to agree with the tray on the machine this was written on,
    ///   which is exactly how it got mistaken for the preference; it does not
    ///   change when the tray selection does.
    /// * `Cache\Servers\DefaultServer` is a legacy machine-wide default that
    ///   does not track the tray menu either — it read `TESTES` while the tray
    ///   showed `CONSISTEM`. It is consulted only when no `PreferredServer` key
    ///   exists anywhere, for an old Caché install that has nothing better.
    fn preferred_server(hive: winreg::HKEY, prefix: &str) -> Option<String> {
        for product in PRODUCTS {
            let path = format!(r"{prefix}\InterSystems\{product}\Configurations");
            let Ok(configs) = RegKey::predef(hive).open_subkey_with_flags(&path, KEY_READ) else {
                continue;
            };
            // Sorted so a machine with several instances — and so several tray
            // icons — resolves the same way on every run rather than following
            // registry enumeration order.
            let mut instances: Vec<String> = configs.enum_keys().flatten().collect();
            instances.sort();
            for instance in instances {
                let Ok(key) = configs.open_subkey_with_flags(
                    format!(r"{instance}\Manager\PreferredServer"),
                    KEY_READ,
                ) else {
                    continue;
                };
                let name = string_value(&key, "");
                if !name.trim().is_empty() {
                    return Some(name.trim().to_string());
                }
            }
        }
        None
    }

    pub fn read() -> ServerList {
        let mut list = ServerList::default();
        let mut legacy_default: Option<String> = None;

        for path in SERVER_KEYS {
            let Ok(root) =
                RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(path, KEY_READ)
            else {
                continue;
            };

            for name in root.enum_keys().flatten() {
                let Ok(key) = root.open_subkey_with_flags(&name, KEY_READ) else {
                    continue;
                };
                // A later key must not shadow an earlier one: the first path
                // that has the server is the one the launcher is using.
                if list
                    .servers
                    .iter()
                    .any(|s| s.name.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
                list.servers.push(Server {
                    address: string_value(&key, "Address"),
                    port: port_value(&key, "Port", 1972),
                    telnet: port_value(&key, "Telnet", 23),
                    comment: string_value(&key, "Comment"),
                    name,
                });
            }

            if legacy_default.is_none() {
                legacy_default = Some(string_value(&root, "DefaultServer"))
                    .filter(|name| !name.trim().is_empty());
            }
        }

        // The tray menu's own choice wins, and the per-user copy of it wins over
        // the one the installer wrote.
        list.preferred = preferred_server(HKEY_CURRENT_USER, "Software")
            .or_else(|| preferred_server(HKEY_CURRENT_USER, r"Software\WOW6432Node"))
            .or_else(|| preferred_server(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node"))
            .or_else(|| preferred_server(HKEY_LOCAL_MACHINE, "SOFTWARE"))
            .or(legacy_default);

        list.servers.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, address: &str) -> Server {
        Server {
            name: name.into(),
            address: address.into(),
            port: 1972,
            telnet: 23,
            comment: String::new(),
        }
    }

    fn list(servers: &[Server], preferred: Option<&str>) -> ServerList {
        ServerList {
            servers: servers.to_vec(),
            preferred: preferred.map(str::to_string),
        }
    }

    /// The whole point of the local case: the preferred server on this machine
    /// is `CONSISTEM` at `localhost`, and opening it must not start asking for
    /// a password the user has never typed here.
    #[test]
    fn a_local_server_matching_an_instance_opens_as_a_local_session() {
        let instances = vec!["CONSISTEM".to_string()];
        assert_eq!(
            server("CONSISTEM", "localhost").target(&instances),
            Target::Local {
                instance: "CONSISTEM".into()
            }
        );
    }

    /// The instance name is what IRIS is started with, so it comes from the
    /// discovered list rather than from the server entry — the two differ in
    /// case often enough, and `irissession` is given what it registered as.
    #[test]
    fn the_instance_name_comes_from_discovery_not_from_the_server_entry() {
        let instances = vec!["CONSISETM".to_string()];
        assert_eq!(
            server("consisetm", "127.0.0.1").target(&instances),
            Target::Local {
                instance: "CONSISETM".into()
            }
        );
    }

    /// A local address with no instance behind it is still a login: there is
    /// nothing to start.
    #[test]
    fn a_local_server_with_no_matching_instance_falls_back_to_telnet() {
        assert_eq!(
            server("TESTES", "localhost").target(&[]),
            Target::Telnet {
                address: "localhost".into(),
                port: 23
            }
        );
    }

    #[test]
    fn a_remote_server_is_always_telnet_on_its_configured_port() {
        let instances = vec!["TESTES".to_string()];
        let mut remote = server("TESTES", "10.0.0.102");
        remote.telnet = 2323;
        assert_eq!(
            remote.target(&instances),
            Target::Telnet {
                address: "10.0.0.102".into(),
                port: 2323
            },
            "a name clash with a local instance must not redirect a remote server"
        );
    }

    /// An entry saved with the address left blank means this machine.
    #[test]
    fn a_blank_address_is_this_machine() {
        assert!(server("X", "").is_local());
        assert_eq!(
            server("X", "").target(&[]),
            Target::Telnet {
                address: "127.0.0.1".into(),
                port: 23
            }
        );
    }

    #[test]
    fn the_preferred_server_is_looked_up_by_name_ignoring_case() {
        let servers = [
            server("CONSISTEM", "localhost"),
            server("TESTES", "10.0.0.102"),
        ];
        let list = list(&servers, Some("testes"));
        assert_eq!(list.preferred().map(|s| s.name.as_str()), Some("TESTES"));
    }

    /// A preference naming a server that has since been deleted must not
    /// resolve to it, and must not resolve to something arbitrary either.
    #[test]
    fn a_preference_for_a_deleted_server_is_ignored() {
        let servers = [
            server("CONSISTEM", "localhost"),
            server("TESTES", "10.0.0.102"),
        ];
        assert!(list(&servers, Some("GONE")).preferred().is_none());
    }

    /// One server is not a choice, so it is the answer whatever the registry
    /// says about preferences.
    #[test]
    fn a_single_server_is_preferred_by_default() {
        let servers = [server("CONSISTEM", "localhost")];
        assert_eq!(
            list(&servers, None).preferred().map(|s| s.name.as_str()),
            Some("CONSISTEM")
        );
        assert!(list(&[], None).preferred().is_none());
    }

    /// What the "+" menu lists: everything except the one the button already
    /// opens.
    #[test]
    fn the_others_are_every_server_but_the_named_one() {
        let servers = [
            server("CONSISTEM", "localhost"),
            server("TESTES", "10.0.0.102"),
        ];
        let list = list(&servers, Some("CONSISTEM"));
        let names: Vec<&str> = list
            .others(Some("consistem"))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["TESTES"]);

        let all: Vec<&str> = list.others(None).map(|s| s.name.as_str()).collect();
        assert_eq!(all, vec!["CONSISTEM", "TESTES"]);
    }

    /// The tray's "Este Servidor" entry names the local instance, not a server.
    /// Dropping it would send the app back to guessing at the first instance it
    /// found, which is only the same answer by luck.
    #[test]
    fn a_preference_naming_a_local_instance_resolves_to_it() {
        let instances = vec!["ALPHA".to_string(), "CONSISTEM".to_string()];
        let list = list(
            &[
                server("CONSISTEM", "localhost"),
                server("TESTES", "10.0.0.102"),
            ],
            Some("ALPHA"),
        );
        assert_eq!(
            preferred_target(&list, &instances),
            Some(Target::Local {
                instance: "ALPHA".into()
            })
        );
    }

    /// The case that was broken in the field: the tray was switched to the
    /// remote server and the app kept opening the local one.
    #[test]
    fn a_preference_for_a_remote_server_resolves_to_telnet() {
        let instances = vec!["CONSISTEM".to_string()];
        let list = list(
            &[
                server("CONSISTEM", "localhost"),
                server("TESTES", "10.0.0.102"),
            ],
            Some("TESTES"),
        );
        assert_eq!(
            preferred_target(&list, &instances),
            Some(Target::Telnet {
                address: "10.0.0.102".into(),
                port: 23
            }),
            "the tray's choice must decide what opens"
        );
    }

    #[test]
    fn an_unresolvable_preference_yields_nothing_to_open() {
        let instances = vec!["CONSISTEM".to_string()];
        let list = list(
            &[
                server("CONSISTEM", "localhost"),
                server("TESTES", "10.0.0.102"),
            ],
            Some("GONE"),
        );
        assert_eq!(preferred_target(&list, &instances), None);
        assert_eq!(preferred_target(&ServerList::default(), &instances), None);
    }

    /// The duplicate the "+" menu was showing: `CONSISTEM` appeared once as a
    /// configured server and again as a discovered instance, and both opened the
    /// same session.
    #[test]
    fn a_local_server_covers_the_instance_it_opens() {
        let instances = vec!["CONSISTEM".to_string()];
        let list = list(
            &[
                server("CONSISTEM", "localhost"),
                server("TESTES", "10.0.0.102"),
            ],
            None,
        );
        assert!(list.covers_instance(&instances, "CONSISTEM"));
        assert!(
            list.covers_instance(&instances, "consistem"),
            "instance names differ in case between the two sources"
        );
        // A remote server named after a local instance opens something else
        // entirely, so it covers nothing.
        assert!(!list.covers_instance(&["TESTES".to_string()], "TESTES"));
        assert!(!list.covers_instance(&instances, "OTHER"));
    }

    /// A discovered instance with no server entry goes through the same path,
    /// and must come out as a local session rather than a Telnet login.
    #[test]
    fn a_bare_instance_entry_opens_locally() {
        let instances = vec!["DESENV".to_string()];
        assert_eq!(
            Server::for_instance("DESENV").target(&instances),
            Target::Local {
                instance: "DESENV".into()
            }
        );
    }

    /// The address is what tells two entries apart when the names do not, and
    /// noise on a local entry is not worth the width.
    #[test]
    fn a_menu_label_names_the_address_only_when_it_is_remote() {
        assert_eq!(server("CONSISTEM", "localhost").menu_label(), "CONSISTEM");
        assert_eq!(
            server("TESTES", "10.0.0.102").menu_label(),
            "TESTES  (10.0.0.102)"
        );
    }
}
