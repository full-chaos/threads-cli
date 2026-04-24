//! threads-cli binary entry point.
//!
//! This Phase-0 stub only prints a banner so the workspace can build
//! end-to-end. Real subcommands (init, auth, ingest, show, search, export)
//! land in Phase 1 Team C.

fn main() {
    println!(
        "threads-cli {} (phase-0 scaffold — commands land in phase-1)",
        env!("CARGO_PKG_VERSION")
    );
}
