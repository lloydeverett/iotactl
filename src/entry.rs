use std::sync::Arc;

use ratatui::style::Color;

use crate::command::Command;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub id: Vec<String>,
    pub is_dir: bool,
    pub is_link: bool,
    pub suggested_commands: Arc<[Command]>,
    /// Nerd Font glyph a source wants shown to the left of this entry's
    /// name, when nerd-font display is enabled. `None` means the source has
    /// no opinion (e.g. an unrecognized file type), in which case the UI
    /// pads with spaces instead of leaving a gap of the wrong width. `char`
    /// and `Color` are both `Copy`, so this stays as cheap as the rest of
    /// `Entry`.
    pub nerd_icon: Option<char>,
    /// Color to draw `nerd_icon` in, if any. Independent of `nerd_icon`
    /// being `Some` — a source could in principle supply an icon with no
    /// color opinion, which falls back to the UI's default text color.
    pub nerd_icon_color: Option<Color>,
}
