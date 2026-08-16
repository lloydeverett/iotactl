#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    pub name: String,
    /// The key that activates this toggle. Carried on the toggle itself so
    /// that generic (non-source-specific) code can wire up key handling
    /// without knowing what the toggle is for.
    pub key: char,
}
