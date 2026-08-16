use std::env;
use std::io::{self, IsTerminal};

/// Prints `error: {msg}` to stderr and exits with status 1. Styled bold red
/// (matching clap's own error style) when stderr is a terminal and `NO_COLOR`
/// isn't set. Used for startup failures that should read like a normal CLI
/// error instead of a Rust panic or a Debug-formatted `io::Error`.
pub fn die(msg: impl std::fmt::Display) -> ! {
    if io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none() {
        eprintln!("\x1b[1m\x1b[31merror:\x1b[0m {msg}");
    } else {
        eprintln!("error: {msg}");
    }
    std::process::exit(1);
}
