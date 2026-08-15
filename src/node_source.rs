use std::io;

use async_trait::async_trait;
use ratatui::text::Text;

use crate::entry::Entry;

/// Abstracts access to a hierarchy of directories and files.
///
/// Methods are async so implementations that need real I/O or heavy
/// computation (e.g. a network-backed source) don't block the render loop.
#[async_trait]
pub trait NodeSource: Send + Sync {
    /// Reads the children of the node at `id`, generally sorted with directories
    /// first, then alphabetically (case-insensitive).
    async fn read_dir(&self, id: &[String], show_hidden: bool) -> io::Result<Vec<Entry>>;

    /// Builds a styled, display-ready preview for the node at `id`.
    async fn preview_tui(&self, id: &[String], show_hidden: bool) -> Text<'static>;
}
