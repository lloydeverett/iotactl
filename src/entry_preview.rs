//! Shared formatting for turning a `NodeSource`'s [`Entry`] list into
//! preview-ready text — what `NodeSource::preview_tui` shows when the
//! previewed node is itself a directory (or an equivalent container).
//! Deliberately independent of any particular source's internals (`fs`
//! is just the first caller) so every implementation can reuse it instead of
//! reinventing directory-listing formatting, and independent of the `ui`
//! module too, since `ui::entry_item` renders the same per-entry styling
//! into `ListItem`s rather than preview text.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::entry::Entry;
use crate::sanitize::SanitizedText;

/// Nerd Font glyph for a directory-like node (a real filesystem directory,
/// a manual category, ...), regardless of name. Shared across sources so
/// every kind of "this node has children" reads the same way, rather than
/// each source picking its own folder glyph. Has no fixed color of its
/// own — a source pairs it with `None` for `nerd_icon_color` so it falls
/// back to whatever color the label text next to it is drawn in.
pub const FOLDER_ICON: char = '\u{f07b}';

/// Builds the Nerd Font icon prefix span for an entry or window title, or
/// `None` when `nerd_font` is off (in which case callers add nothing,
/// rather than reserving icon-sized blank space, so the plain-text UI is
/// unchanged from before this feature existed).
///
/// `color` is the icon's own color, if the source expressed one; when it
/// didn't (`None`), `fallback_color` is used instead — e.g. a directory
/// icon with no color of its own inherits whatever color the label text
/// next to it is drawn in, rather than a fixed default. When `icon` itself
/// is `None` (no icon at all), `pad_when_missing` decides what happens:
/// true pads with two blank spaces so names still line up against rows
/// that do have an icon (listings); false omits the span entirely, since a
/// window title has no column of icons to line up against.
pub fn nerd_icon_span(
    nerd_font: bool,
    icon: Option<char>,
    color: Option<Color>,
    fallback_color: Option<Color>,
    pad_when_missing: bool,
) -> Option<Span<'static>> {
    if !nerd_font {
        return None;
    }
    let text = match icon {
        Some(c) => format!("{c} "),
        None if pad_when_missing => "  ".to_string(),
        None => return None,
    };
    let style = match color.or(fallback_color) {
        Some(c) => Style::default().fg(c),
        None => Style::default(),
    };
    Some(Span::styled(text, style))
}

/// The label (name plus directory/link decoration) and style for a single
/// entry — shared by the column listing (`ui::entry_item`) and directory
/// preview (`format_dir_preview`) so both render entries identically.
pub fn entry_label(entry: &Entry) -> (String, Style) {
    if entry.is_dir {
        (
            format!("{}/", entry.name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if entry.is_link {
        (
            format!("{}@", entry.name),
            Style::default().fg(Color::Magenta),
        )
    } else {
        (entry.name.clone(), Style::default())
    }
}

/// Builds one preview line for `entry`: its sanitized, styled label (see
/// `entry_label`), preceded by its Nerd Font icon when `nerd_font` is on.
/// An icon with no color of its own (e.g. a folder, which lets the UI paint
/// it whatever color directory names are drawn in) falls back to the
/// label's own color rather than a fixed default, so the two stay in sync
/// without this module needing to duplicate the source's color choice.
pub fn entry_line(entry: &Entry, nerd_font: bool) -> Line<'static> {
    let (label, style) = entry_label(entry);
    let mut line = SanitizedText::from_label(&label, style);
    if let Some(icon) = nerd_icon_span(
        nerd_font,
        entry.nerd_icon,
        entry.nerd_icon_color,
        style.fg,
        true,
    ) {
        line.spans.insert(0, icon);
    }
    line
}

/// Formats `entries` as a preview: one line per entry via `entry_line`, or
/// a dim "empty directory" placeholder if there are none. Any `NodeSource`
/// impl can call this from `preview_tui` when the previewed node is itself
/// a directory — see `fs::FsSource::preview_tui_sync` for the
/// canonical example.
pub fn format_dir_preview(entries: &[Entry], nerd_font: bool) -> SanitizedText {
    if entries.is_empty() {
        return SanitizedText::from_text("empty directory", Style::default().fg(Color::DarkGray));
    }
    let lines = entries.iter().map(|e| entry_line(e, nerd_font)).collect();
    SanitizedText::assume_sanitized(lines)
}
