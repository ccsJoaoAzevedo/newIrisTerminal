//! Every integration test, in one binary.
//!
//! One file per binary meant one link per file, and fifteen linkers running at
//! once took ~250 MB each - enough to make the machine unusable during a
//! `cargo test`. As modules of a single crate they cost one link.
//!
//! Run one by its module path:
//!
//! ```text
//! cargo test --test integration live_session:: -- --ignored --nocapture
//! ```
//!
//! `resize_memory` installs a counting global allocator, which now covers the
//! whole binary. That costs the other tests one relaxed atomic add per
//! allocation, and its own number stays honest only when it is run alone,
//! filtered by name as above.

mod encoding_probe;
mod live_break_prompt;
mod live_charset;
mod live_clear;
mod live_input;
mod live_resize;
mod live_servers;
mod live_session;
mod live_shell;
mod live_timing;
mod live_update;
mod live_width;
mod live_wrap;
mod paint_cost;
mod resize_memory;
