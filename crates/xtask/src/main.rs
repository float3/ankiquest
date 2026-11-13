//! Repository tools, run with `cargo run -p ankiquest-xtask -- <command>`.
//!
//! - `android-contract --android-resources <dir> [--server-examples <dir>]`

mod contract;

use std::env;
use std::process::ExitCode;

const USAGE: &str = "usage: xtask android-contract --android-resources <dir> \
[--server-examples <dir>]";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    // Dispatch to the command; each parses its own flags.
    match command.as_deref() {
        Some("android-contract") => contract::run(&rest),
        _ => usage(),
    }
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::FAILURE
}
