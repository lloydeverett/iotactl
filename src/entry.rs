#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub id: Vec<String>,
    pub is_dir: bool,
    pub is_symlink: bool,
}
