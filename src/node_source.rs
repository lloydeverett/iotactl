use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::command::Command;
use crate::entry::Entry;
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

/// A cheap, cloneable "has this call been superseded?" signal passed into
/// [`NodeSource::preview_tui`]. The caller (`App`) flips it via `cancel()`
/// once a newer request has made the in-flight one moot — e.g. the user
/// moved the selection again before the previous preview finished loading.
///
/// Checking it is purely advisory: `App` still discards a result that
/// arrives after cancellation (by epoch, separately from this flag), so a
/// source is free to ignore `Cancelled` entirely and just run to
/// completion. But for work with a natural per-iteration checkpoint — e.g.
/// scanning a large directory entry by entry — checking `is_cancelled()`
/// there lets the source bail out early instead of finishing an expensive
/// computation whose result was already known to be thrown away. It won't
/// help work that has no such checkpoint, like a single bounded read.
#[derive(Clone, Default)]
pub struct Cancelled(Arc<AtomicBool>);

impl Cancelled {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the call this flag was handed to as superseded.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether `cancel` has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What [`NodeSource::preview_tui`] returns: the styled preview text, plus
/// whatever the source knows about how it should be displayed that the UI
/// couldn't infer on its own.
pub struct Preview {
    pub text: SanitizedText,
    /// Forces the preview pane's line-number gutter off regardless of the
    /// user's line-numbers toggle. Set this when line numbers wouldn't
    /// correspond to anything meaningful in `text` — e.g. a directory
    /// listing or a `key: value` metadata dump rather than file content.
    pub override_disable_line_numbers: bool,
}

impl Preview {
    pub fn new(text: SanitizedText) -> Self {
        Preview {
            text,
            override_disable_line_numbers: false,
        }
    }
}

/// Abstracts access to a hierarchy of directories and files.
///
/// Methods are async so implementations that need real I/O or heavy
/// computation (e.g. a network-backed source) don't block the render loop.
#[async_trait]
pub trait NodeSource: Send + Sync {
    /// Reads the children of the node at `id`, generally sorted with directories
    /// first, then alphabetically (case-insensitive). Any filtering of the
    /// result (e.g. hidden entries) is entirely up to the source, driven by
    /// whatever toggle state it holds internally.
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>>;

    /// Entry describing the root node itself (`id == []`). Used for the
    /// initial column's display name and icon, since the root has no
    /// parent entry that could otherwise supply that information — every
    /// other column's title icon is looked up from the `Entry` in its
    /// *parent* column that opened it (see `App::column_icon`), which
    /// doesn't work for the root.
    async fn root_entry(&self) -> Entry;

    /// Builds a styled, display-ready preview for the node at `id`. Carries
    /// `SanitizedText` rather than a raw `Text` so every implementation is
    /// forced through the escaping in [`crate::sanitize`] — see that
    /// module's docs for why that matters.
    ///
    /// `cancelled` is flipped once this call is superseded by a newer one
    /// (e.g. the user already moved the selection again). Implementations
    /// with a natural checkpoint for expensive work — a loop over many
    /// entries, a chunked read, a paginated network fetch — are encouraged
    /// to check it there and bail out early wherever that's practical; it's
    /// purely advisory, and it's fine to ignore it when there's no sensible
    /// place to check.
    async fn preview_tui(&self, id: &[String], cancelled: &Cancelled) -> Preview;

    /// The commands this source makes available, independent of any
    /// particular node. Not yet invoked anywhere.
    fn available_commands(&self) -> Arc<[Command]>;

    /// The toggles this source makes available, independent of any
    /// particular node. Not yet invoked anywhere.
    fn available_toggles(&self) -> Arc<[Toggle]>;

    /// Sets `toggle` to `value`. Not yet invoked anywhere.
    async fn set_toggle(&self, toggle: &Toggle, value: bool) -> io::Result<()>;

    /// Reads the current value of `toggle`. Not yet invoked anywhere.
    async fn get_toggle(&self, toggle: &Toggle) -> io::Result<bool>;

    /// Runs `command` with `args`. Not yet invoked anywhere.
    async fn execute_command(&self, command: &Command, args: &[String]) -> io::Result<()>;
}
