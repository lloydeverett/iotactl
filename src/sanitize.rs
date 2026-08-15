//! Sanitizes untrusted text — file contents, filenames, anything that
//! ultimately comes from outside the program — before it reaches the UI.
//!
//! `ratatui::widgets::Paragraph` sends grapheme content straight to the
//! terminal without filtering control characters the way
//! `Buffer::set_stringn` does elsewhere in ratatui (a still-open gap:
//! <https://github.com/ratatui/ratatui/issues/876>). A raw tab or stray
//! control byte reaching the terminal desyncs its cursor from ratatui's own
//! column bookkeeping and corrupts the screen — and, because the damage is
//! a real terminal-state artifact rather than something ratatui's diffed
//! redraw knows to undo, it can outlive the frame that caused it.
//!
//! [`SanitizedText`] exists so that can't happen: it's the type
//! `NodeSource::preview_tui` returns, and the only ordinary way to build
//! one is by running text through this module's escaping.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};

/// Number of columns a tab advances to the next stop.
const TAB_WIDTH: usize = 4;

/// Display-ready text, guaranteed free of raw control characters: tabs are
/// expanded to spaces, and everything else in Unicode's `Cc` (control)
/// category is escaped. Safe to hand straight to a ratatui `Paragraph` (or
/// any other widget) without risking terminal corruption.
#[derive(Debug, Clone, Default)]
pub struct SanitizedText(Text<'static>);

impl SanitizedText {
    /// Sanitizes `text` as multi-line content: `\n` starts a new line, and
    /// every other control character (tabs excepted, which get expanded)
    /// is escaped. `style` is applied to the non-escaped portions.
    pub fn from_text(text: &str, style: Style) -> Self {
        SanitizedText(Text::from(sanitize_lines(text, style, true)))
    }

    /// Sanitizes `text` as a single-line label (e.g. a filename): behaves
    /// like `from_text`, except `\n`/`\r` are escaped rather than treated
    /// as a line break, since callers use this where the result must stay
    /// on one visual line.
    pub fn from_label(text: &str, style: Style) -> Line<'static> {
        sanitize_lines(text, style, false)
            .into_iter()
            .next()
            .expect("sanitize_lines always returns at least one line")
    }

    /// Wraps already-built lines as sanitized, *without* performing any
    /// escaping or checking.
    ///
    /// # Danger
    /// Only reach for this when `lines` is independently known to already
    /// be free of raw control characters — e.g. because it was built by a
    /// tokenizer (like a syntax highlighter) that only re-slices and
    /// re-styles text that already went through [`SanitizedText::from_text`],
    /// without introducing any characters of its own. Handing this
    /// arbitrary or unvalidated text reopens exactly the terminal
    /// corruption bug this type exists to prevent — see the module docs.
    pub fn assume_sanitized(lines: Vec<Line<'static>>) -> Self {
        SanitizedText(Text::from(lines))
    }

    /// Flattens back to a plain string (lines joined by `\n`) — e.g. to
    /// feed a syntax highlighter, which should tokenize exactly the text
    /// that ends up on screen (tabs expanded, control chars escaped) so
    /// its output stays aligned with what was actually sanitized.
    pub fn plain(&self) -> String {
        self.0
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn line_count(&self) -> usize {
        self.0.lines.len()
    }
}

impl From<SanitizedText> for Text<'static> {
    fn from(sanitized: SanitizedText) -> Self {
        sanitized.0
    }
}

/// Renders a control character (other than tab, which callers expand
/// separately) as a printable escape. C0 controls and DEL use the classic
/// caret notation (`^I`, `^[`, `^?`, ...: the code point XORed with
/// `0x40`); the rarer C1 block (`0x80..=0x9F`) has no caret convention, so
/// it falls back to `<XX>` hex, matching vim's behavior for that range.
fn caret_escape(c: char) -> String {
    match c as u32 {
        0x7F => "^?".to_string(),
        0x00..=0x1F => format!("^{}", ((c as u8) ^ 0x40) as char),
        code => format!("<{code:02X}>"),
    }
}

/// Core sanitizer shared by [`SanitizedText::from_text`] and
/// [`SanitizedText::from_label`]. When `split_on_newline` is true, `\n` breaks
/// lines (multi-line file content); otherwise every character, including
/// `\n`/`\r`, is escaped like any other control character (single-line
/// labels), and exactly one `Line` is returned.
fn sanitize_lines(text: &str, style: Style, split_on_newline: bool) -> Vec<Line<'static>> {
    let escape_style = Style::default().fg(Color::DarkGray);
    let raw_lines: Vec<&str> = if split_on_newline {
        text.split('\n').collect()
    } else {
        vec![text]
    };
    raw_lines
        .into_iter()
        .map(|line| {
            let mut spans = Vec::new();
            let mut plain = String::new();
            let mut col = 0usize;
            for c in line.chars() {
                if c == '\t' {
                    let width = TAB_WIDTH - (col % TAB_WIDTH);
                    plain.extend(std::iter::repeat_n(' ', width));
                    col += width;
                } else if c.is_control() {
                    if !plain.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut plain), style));
                    }
                    let escaped = caret_escape(c);
                    col += escaped.chars().count();
                    spans.push(Span::styled(escaped, escape_style));
                } else {
                    plain.push(c);
                    col += 1;
                }
            }
            if !plain.is_empty() || spans.is_empty() {
                spans.push(Span::styled(plain, style));
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tabs_to_next_stop() {
        let sanitized = SanitizedText::from_text("a\tb", Style::default());
        assert_eq!(sanitized.plain(), "a   b");
    }

    #[test]
    fn escapes_c0_controls_with_caret_notation() {
        let sanitized = SanitizedText::from_text("a\x01b\x1bc\x7fd", Style::default());
        assert_eq!(sanitized.plain(), "a^Ab^[c^?d");
    }

    #[test]
    fn escapes_c1_controls_with_hex_notation() {
        let sanitized = SanitizedText::from_text("a\u{80}b", Style::default());
        assert_eq!(sanitized.plain(), "a<80>b");
    }

    #[test]
    fn splits_multiline_text_on_newline() {
        let sanitized = SanitizedText::from_text("one\ntwo", Style::default());
        assert_eq!(sanitized.line_count(), 2);
        assert_eq!(sanitized.plain(), "one\ntwo");
    }

    #[test]
    fn label_escapes_embedded_newline_instead_of_breaking_line() {
        let line = SanitizedText::from_label("weird\nname", Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "weird^Jname");
    }

    #[test]
    fn escape_spans_are_dimmed() {
        let sanitized = SanitizedText::from_text("a\x01b", Style::default());
        let text: Text<'static> = sanitized.into();
        let escape_span = text.lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "^A")
            .expect("expected an escaped span for \\x01");
        assert_eq!(escape_span.style.fg, Some(Color::DarkGray));
    }
}
