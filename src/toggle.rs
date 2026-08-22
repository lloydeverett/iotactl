#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toggle {
    /// Static because every `Toggle` is one of a node source type's fixed,
    /// compile-time-known `toggles` (see `crate::node_source::NodeSourceType`)
    /// — never built dynamically from user or file data.
    pub name: &'static str,
    /// The key that activates this toggle. Carried on the toggle itself so
    /// that generic (non-source-specific) code can wire up key handling
    /// without knowing what the toggle is for.
    pub key: char,
}
