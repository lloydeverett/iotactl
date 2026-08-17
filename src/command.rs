#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    /// Static for the same reason as `Toggle::name` — see its doc comment.
    pub name: &'static str,
}
