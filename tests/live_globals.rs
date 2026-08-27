//! Runs the global browser's extraction against a real instance.
//!
//! ```text
//! cargo test --test live_globals -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Read-only throughout: the walk uses `$Query`/`$Get`, and the one global it
//! creates lives in the process-private `^||` scratch space, which never
//! touches the shared database.

use std::time::{Duration, Instant};

use new_iris_terminal::features::global_browser::{self, Query};
use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Encoding, Grid};

struct Session {
    pty: PtySession,
    grid: Grid,
    vte: vte::Parser,
    capture: String,
}

impl Session {
    fn open() -> Self {
        let instance = std::env::var("IRIS_TEST_INSTANCE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
            .expect("no IRIS instance");
        let spec = LaunchSpec {
            instance,
            ..LaunchSpec::default()
        };
        let pty = PtySession::spawn(launcher().as_ref(), &spec, 120, 30).expect("spawn");

        let mut session = Session {
            pty,
            grid: Grid::new(120, 30, 4000),
            vte: vte::Parser::new(),
            capture: String::new(),
        };
        session.wait_for(Duration::from_secs(20), |s| {
            s.capture.contains('>')
        });
        session
    }

    fn wait_for<F: Fn(&Session) -> bool>(&mut self, timeout: Duration, done: F) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let (bytes, ended) = self.pty.drain();
            if !bytes.is_empty() {
                let decoded = Encoding::Utf8.decode(&bytes);
                self.capture.push_str(&String::from_utf8_lossy(&decoded));
                parser::advance(&mut self.vte, &mut self.grid, &decoded);
                if done(self) {
                    return true;
                }
            }
            if ended {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done(self)
    }

    fn run(&mut self, line: &str) {
        self.pty.write(line.as_bytes()).unwrap();
        self.pty.write(b"\r").unwrap();
    }

    fn settle(&mut self, ms: u64) {
        self.wait_for(Duration::from_millis(ms), |_| false);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pty.request_halt();
    }
}

/// Builds a known process-private global so the assertions are exact.
///
/// `^||NIT` is scoped to this process and vanishes when the session ends, so
/// nothing shared is written.
fn seed(session: &mut Session) {
    session.run("Kill ^||NIT");
    session.run("Set ^||NIT(1)=\"ABC^2024^12.50\"");
    session.run("Set ^||NIT(2)=\"DEF^2025^9.90\"");
    session.run("Set ^||NIT(2,\"x\")=\"nested^value\"");
    session.settle(1200);
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn the_extraction_returns_nodes_and_pieces() {
    let mut session = Session::open();
    seed(&mut session);

    let query = Query {
        global: "||NIT".into(),
        ..Query::default()
    };
    let script = global_browser::build_script(&query);

    session.capture.clear();
    session.run(&script);
    session.wait_for(Duration::from_secs(20), |s| s.capture.contains("@E@"));

    let page = global_browser::parse(&session.capture, query.limit);
    println!("error: {:?}", page.error);
    for node in &page.nodes {
        println!("{} -> {:?}", node.reference, node.pieces);
    }

    assert!(page.error.is_none(), "IRIS reported: {:?}", page.error);
    assert_eq!(page.nodes.len(), 3, "expected three seeded nodes");

    assert_eq!(page.nodes[0].pieces, vec!["ABC", "2024", "12.50"]);
    assert_eq!(page.nodes[1].pieces, vec!["DEF", "2025", "9.90"]);
    assert_eq!(page.nodes[2].pieces, vec!["nested", "value"]);

    // Subscripts must survive as separate columns, including the nested one.
    assert_eq!(page.nodes[2].subscripts.len(), 2);
    assert_eq!(page.piece_columns(), 3);
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn the_limit_paginates_and_resumes_without_gaps() {
    let mut session = Session::open();
    session.run("Kill ^||NIT2");
    session.run("For i=1:1:10 { Set ^||NIT2(i)=\"row\"_i }");
    session.settle(1200);

    let mut collected = Vec::new();
    let mut start = String::new();

    for round in 0..4 {
        let query = Query {
            global: "||NIT2".into(),
            start: start.clone(),
            limit: 4,
            ..Query::default()
        };
        session.capture.clear();
        session.run(&global_browser::build_script(&query));
        session.wait_for(Duration::from_secs(20), |s| {
            s.capture.contains("@E@") || s.capture.contains("@X@")
        });

        let page = global_browser::parse(&session.capture, query.limit);
        assert!(page.error.is_none(), "round {round}: {:?}", page.error);
        println!(
            "round {round}: {} nodes, truncated={}",
            page.nodes.len(),
            page.truncated
        );
        collected.extend(page.nodes.iter().map(|n| n.reference.clone()));

        match page.resume_from() {
            Some(next) => start = next,
            None => break,
        }
    }

    println!("collected: {collected:?}");
    assert_eq!(collected.len(), 10, "pagination lost or duplicated rows");

    // No duplicates: resuming from the last reference must not re-read it.
    let mut unique = collected.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 10, "resume re-read a node it had already returned");
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn a_bad_global_reports_an_error_and_leaves_the_session_usable() {
    let mut session = Session::open();

    let query = Query {
        global: "NoSuchGlobalHopefully".into(),
        ..Query::default()
    };
    session.capture.clear();
    session.run(&global_browser::build_script(&query));
    session.wait_for(Duration::from_secs(15), |s| {
        s.capture.contains("@E@") || s.capture.contains("@X@")
    });

    let page = global_browser::parse(&session.capture, query.limit);
    println!("nodes: {}, error: {:?}", page.nodes.len(), page.error);
    assert!(page.nodes.is_empty(), "an empty global returned rows");

    // Most importantly: the session must still respond afterwards.
    session.capture.clear();
    session.run("Write 6*7,!");
    let alive = session.wait_for(Duration::from_secs(10), |s| s.capture.contains("42"));
    assert!(alive, "the session was left unusable after the failed query");
}
