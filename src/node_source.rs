use std::io;

use ratatui::text::Text;

use crate::entry::Entry;

/// Abstracts access to a hierarchy of directories and files.
pub trait NodeSource {
    /// Reads the children of the node at `id`, generally sorted with directories
    /// first, then alphabetically (case-insensitive).
    fn read_dir(&self, id: &[String], show_hidden: bool) -> io::Result<Vec<Entry>>;

    /// Builds a styled, display-ready preview for the node at `id`.
    fn preview_tui(&self, id: &[String], show_hidden: bool) -> Text<'static>;
}
