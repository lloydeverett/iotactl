//! Immutable, process-wide configuration set once by `main` from parsed CLI
//! flags at startup, and read from anywhere in the crate after that.

use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();
static ALLOW_SLOW_PIPES: OnceLock<bool> = OnceLock::new();
static SLOW_PIPE_BUFFER_SIZE: OnceLock<usize> = OnceLock::new();

/// Sets the flags below. Must be called exactly once, before any getter here
/// is called — `main` does this immediately after parsing CLI args, ahead of
/// constructing anything that might read them.
pub fn init(nerd_font: bool, allow_slow_pipes: bool, slow_pipe_buffer_size: usize) {
    NERD_FONT
        .set(nerd_font)
        .expect("config::init called more than once");
    ALLOW_SLOW_PIPES
        .set(allow_slow_pipes)
        .expect("config::init called more than once");
    SLOW_PIPE_BUFFER_SIZE
        .set(slow_pipe_buffer_size)
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

/// How many bytes of lookback buffer simulated seeking (see
/// `crate::streams::simulated_seeking`) keeps for each stream. A seek
/// backward that lands within this many bytes of the furthest point
/// streamed so far replays those buffered bytes instead of restarting the
/// stream from scratch. Panics if [`init`] hasn't been called yet.
pub fn slow_pipe_buffer_size() -> usize {
    *SLOW_PIPE_BUFFER_SIZE
        .get()
        .expect("config::slow_pipe_buffer_size called before config::init")
}

/// Calls [`init`] with fixed test values, the first time this is called —
/// `main` isn't in the loop in a `cargo test` binary, so any test exercising
/// code that reads these flags has to seed them itself. Every such test
/// across the binary shares this one call (via `Once`) rather than each
/// calling [`init`] itself, since it panics if called more than once and
/// tests in this binary run concurrently. `slow_pipe_buffer_size` is
/// deliberately tiny (2 bytes): small enough that tests can exercise both a
/// seek that's satisfied from the buffer and one that has to restart.
#[cfg(test)]
pub(crate) fn ensure_initialized_for_tests() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| init(false, true, 2));
}
