use std::io;
use std::sync::Arc;

use async_trait::async_trait;

use crate::command::Command;
use crate::entry::Entry;
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

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

    /// Builds a styled, display-ready preview for the node at `id`. Returns
    /// `SanitizedText` rather than a raw `Text` so every implementation is
    /// forced through the escaping in [`crate::sanitize`] — see that
    /// module's docs for why that matters.
    async fn preview_tui(&self, id: &[String]) -> SanitizedText;

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
