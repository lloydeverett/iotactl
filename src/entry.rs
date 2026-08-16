use std::sync::Arc;

use crate::command::Command;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub id: Vec<String>,
    pub is_dir: bool,
    pub is_link: bool,
    pub suggested_commands: Arc<[Command]>,
}
