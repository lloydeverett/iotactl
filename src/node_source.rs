use std::io;

use async_trait::async_trait;

use crate::entry::Entry;
use crate::sanitize::SanitizedText;

/// Abstracts access to a hierarchy of directories and files.
///
/// Methods are async so implementations that need real I/O or heavy
/// computation (e.g. a network-backed source) don't block the render loop.
#[async_trait]
pub trait NodeSource: Send + Sync {
    /// Reads the children of the node at `id`, generally sorted with directories
    /// first, then alphabetically (case-insensitive).
    async fn read_dir(&self, id: &[String], show_hidden: bool) -> io::Result<Vec<Entry>>;

    /// Builds a styled, display-ready preview for the node at `id`. Returns
    /// `SanitizedText` rather than a raw `Text` so every implementation is
    /// forced through the escaping in [`crate::sanitize`] — see that
    /// module's docs for why that matters.
    async fn preview_tui(&self, id: &[String], show_hidden: bool) -> SanitizedText;
}
