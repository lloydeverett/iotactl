//! Immutable, process-wide configuration set once by `main` from parsed CLI
//! flags at startup, and read from anywhere in the crate after that.

use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();

/// Sets the flags below. Must be called exactly once, before any getter here
/// is called — `main` does this immediately after parsing CLI args, ahead of
/// constructing anything that might read them.
pub fn init(nerd_font: bool) {
    NERD_FONT
        .set(nerd_font)
        .expect("config::init called more than once");
}

/// Whether Nerd Font icons should be shown. Panics if [`init`] hasn't been
/// called yet.
pub fn nerd_font() -> bool {
    *NERD_FONT
        .get()
        .expect("config::nerd_font called before config::init")
}
