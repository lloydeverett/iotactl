//! Immutable, process-wide configuration set once by `main` from parsed CLI
//! flags at startup, and read from anywhere in the crate after that.

use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();
static ALLOW_SLOW_PIPES: OnceLock<bool> = OnceLock::new();

/// Sets the flags below. Must be called exactly once, before any getter here
/// is called — `main` does this immediately after parsing CLI args, ahead of
/// constructing anything that might read them.
pub fn init(nerd_font: bool, allow_slow_pipes: bool) {
    NERD_FONT
        .set(nerd_font)
        .expect("config::init called more than once");
    ALLOW_SLOW_PIPES
        .set(allow_slow_pipes)
        .expect("config::init called more than once");
}

/// Whether Nerd Font icons should be shown. Panics if [`init`] hasn't been
/// called yet.
pub fn nerd_font() -> bool {
    *NERD_FONT
        .get()
        .expect("config::nerd_font called before config::init")
}

/// Whether a node source may fake random access over a stream that can only
/// really be read forward, once, from its start (see
/// `crate::streams::simulated_seeking`). Off by default: for some pipe
/// configurations (e.g. a zip archive nested inside another zip archive)
/// that fakery means redoing real, potentially expensive work — repeated
/// decompression — on every seek backward, a cost `--allow-slow-pipes`
/// opts into rather than one a node source should silently impose. Panics
/// if [`init`] hasn't been called yet.
pub fn allow_slow_pipes() -> bool {
    *ALLOW_SLOW_PIPES
        .get()
        .expect("config::allow_slow_pipes called before config::init")
}
