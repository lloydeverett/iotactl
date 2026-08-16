use std::sync::Arc;

use crate::command::Command;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub id: Vec<String>,
    pub is_dir: bool,
    pub is_link: bool,
    /// Commands available on this entry. Shared rather than owned per-entry
    /// since many entries from the same source typically expose the same
    /// set of commands.
    pub commands: Arc<[Command]>,
}
