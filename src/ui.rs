use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::entry::Entry;
use crate::entry_preview::{entry_label, nerd_icon_span};
use crate::sanitize::SanitizedText;

/// Width of a non-focused opened directory column, in terminal cells.
const COLUMN_WIDTH: u16 = 24;
/// Width of the focused directory column. Wider than the others so the
/// column you're actively navigating gets more room.
const FOCUSED_COLUMN_WIDTH: u16 = 40;
/// Minimum width reserved for the trailing preview column.
const PREVIEW_MIN_WIDTH: u16 = 24;
/// Minimum width reserved for the preview column when it's focused.
const PREVIEW_FOCUSED_MIN_WIDTH: u16 = 80;

pub fn draw(f: &mut Frame, app: &mut App) {
    let footer = footer_text(app, f.area().width);
    let footer_height = footer.lines.len().max(1) as u16;
    let (body, footer_rect) = root_layout(f.area(), footer_height);

    draw_columns(f, body, app);
    f.render_widget(Paragraph::new(footer), footer_rect);
}

/// Splits the full terminal area into the body (columns + preview) and the
/// footer, given the footer's already-computed height. The one place this
/// split happens, so `draw` and the mouse hit-testing below always agree on
/// where the body ends and the footer begins.
fn root_layout(term_area: Rect, footer_height: u16) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(footer_height)])
        .split(term_area);
    (chunks[0], chunks[1])
}

/// Recomputes just the body `Rect` (the part `root_layout` calls `chunks[0]`)
/// for a terminal of `term_area`, for use by mouse hit-testing outside of a
/// render pass.
fn body_area(term_area: Rect, app: &App) -> Rect {
    let footer = footer_text(app, term_area.width);
    let footer_height = footer.lines.len().max(1) as u16;
    root_layout(term_area, footer_height).0
}

fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// What a left-click landed on, as resolved by `hit_test`.
pub enum MouseTarget {
    /// A row in `app.columns[col_idx]` at `row_idx`.
    Entry { col_idx: usize, row_idx: usize },
    /// Inside `app.columns[col_idx]`'s box (its border/title, or blank
    /// space past the last entry) but not on any particular row. Only
    /// produced for a column other than the focused one — see `hit_test`.
    Column { col_idx: usize },
    /// Anywhere in the preview pane.
    Preview,
}

/// Resolves a click at terminal coordinates `(x, y)` to whatever it landed
/// on, using exactly the same layout math `draw` used to put things there
/// (see `column_layout`) rather than tracking `Rect`s left over from the
/// last render.
pub fn hit_test(term_area: Rect, app: &App, x: u16, y: u16) -> Option<MouseTarget> {
    let body = body_area(term_area, app);
    if !rect_contains(body, x, y) {
        return None;
    }
    if app.preview_focused && app.zoom_preview {
        return Some(MouseTarget::Preview);
    }

    let layout = column_layout(body, app);
    let last_col_idx = app.columns.len() - 1;
    for (col_idx, rect) in layout.columns {
        if rect_contains(rect, x, y) {
            if let Some(target) = hit_entry(app, col_idx, rect, y) {
                return Some(target);
            }
            // Missed the item hitbox. For any column other than the
            // focused one, still move focus there — clicking a non-focused
            // column's border/title or its blank space below the last
            // entry is a reasonable way to jump to it without aiming for a
            // specific row.
            let is_focused = col_idx == last_col_idx && !app.preview_focused;
            return (!is_focused).then_some(MouseTarget::Column { col_idx });
        }
    }
    rect_contains(layout.preview, x, y).then_some(MouseTarget::Preview)
}

/// Whether `(x, y)` falls within the preview pane, for scroll-wheel routing
/// (which doesn't need to know *what* was hit, just whether it should scroll
/// the preview vs. the focused column).
pub fn point_in_preview(term_area: Rect, app: &App, x: u16, y: u16) -> bool {
    let body = body_area(term_area, app);
    if app.preview_focused && app.zoom_preview {
        return rect_contains(body, x, y);
    }
    rect_contains(column_layout(body, app).preview, x, y)
}

/// Maps a click at row `y` inside a column's bordered box (`rect`) to an
/// entry index, using that column's list state — persisted across frames
/// (see `Column::list_state`) so its `offset()` matches what was actually
/// last drawn. Returns `None` for a click on the border/title, or past the
/// last entry (e.g. in the blank space below a short list).
fn hit_entry(app: &App, col_idx: usize, rect: Rect, y: u16) -> Option<MouseTarget> {
    let column = &app.columns[col_idx];
    if column.loading {
        return None;
    }
    if y <= rect.y || y >= rect.y + rect.height.saturating_sub(1) {
        return None;
    }
    let row_in_view = (y - rect.y - 1) as usize;
    let row_idx = row_in_view + column.list_state.offset();
    if row_idx >= column.entries.len() {
        return None;
    }
    Some(MouseTarget::Entry { col_idx, row_idx })
}

/// The `Rect`s `draw_columns` renders into: each visible column tagged with
/// its real index into `app.columns`, plus the trailing preview pane. Split
/// out from `draw_columns` so mouse hit-testing can compute the exact same
/// layout without a render pass.
struct ColumnLayout {
    columns: Vec<(usize, Rect)>,
    preview: Rect,
}

fn column_layout(area: Rect, app: &App) -> ColumnLayout {
    let total = app.columns.len();

    // The last column is always visible; it's the focused one unless focus
    // has moved to the preview pane.
    let last_column_width = if app.preview_focused {
        COLUMN_WIDTH
    } else {
        FOCUSED_COLUMN_WIDTH
    };

    let preview_min_width = if app.preview_focused {
        PREVIEW_FOCUSED_MIN_WIDTH
    } else {
        PREVIEW_MIN_WIDTH
    };
    let preview_width = preview_min_width.min(area.width);
    let available_for_columns = area.width.saturating_sub(preview_width);
    let mut remaining = available_for_columns.saturating_sub(last_column_width);
    let mut visible = 1usize.min(total);
    while visible < total && remaining >= COLUMN_WIDTH {
        remaining -= COLUMN_WIDTH;
        visible += 1;
    }
    let start_idx = total - visible;

    let mut constraints: Vec<Constraint> = Vec::with_capacity(visible + 1);
    for offset in 0..visible {
        let col_idx = start_idx + offset;
        let width = if col_idx == total - 1 {
            last_column_width
        } else {
            COLUMN_WIDTH
        };
        constraints.push(Constraint::Length(width));
    }
    constraints.push(Constraint::Min(preview_width));
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let columns = (0..visible)
        .map(|offset| (start_idx + offset, chunks[offset]))
        .collect();
    ColumnLayout {
        columns,
        preview: chunks[visible],
    }
}

/// Border/title style for a box, depending on whether it holds the
/// currently focused column.
fn box_style(is_focused: bool) -> (Style, Style) {
    if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Gray),
        )
    }
}

fn titled_box(
    icon: (Option<char>, Option<Color>),
    title: String,
    is_focused: bool,
) -> Block<'static> {
    let (border_style, title_style) = box_style(is_focused);
    let mut spans = vec![Span::styled(" ", title_style)];
    let nerd_font = crate::config::nerd_font();
    // `pad_when_missing: false` — a title has no column of icons next to it
    // to line up against, so an entry with no icon opinion just omits the
    // span rather than leaving a blank gap.
    if let Some(icon_span) = nerd_icon_span(nerd_font, icon.0, icon.1, None, false) {
        spans.push(icon_span);
    }
    // `title` is an entry/directory name straight from the OS (see
    // `Entry::name`), so it's routed through `SanitizedText::from_label`
    // like any other filename before it reaches a `Span` — otherwise a
    // raw tab or control character in it would bypass the sanitizer this
    // whole codebase relies on to avoid corrupting the terminal.
    spans.extend(SanitizedText::from_label(&title, title_style).spans);
    spans.push(Span::styled(" ", title_style));
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Line::from(spans))
}

/// Renders the Miller-columns stack of opened directories, followed by a
/// preview column. When there isn't room for every opened column, the
/// oldest (leftmost) ones slide out of view so the focused column and
/// preview stay visible.
fn draw_columns(f: &mut Frame, area: Rect, app: &mut App) {
    // Zoomed preview replaces the whole column stack rather than just
    // widening the preview pane's share of it, so it can go back to
    // occupying the same footprint the moment focus leaves the preview
    // (e.g. pressing `h`) without needing to unwind any layout state.
    if app.preview_focused && app.zoom_preview {
        draw_preview_column(f, area, app);
        return;
    }

    let total = app.columns.len();
    let layout = column_layout(area, app);

    for (col_idx, rect) in layout.columns {
        let is_focused = col_idx == total - 1 && !app.preview_focused;
        let column = &app.columns[col_idx];

        let block = titled_box(
            app.column_icon(col_idx),
            app.column_label(col_idx),
            is_focused,
        );

        if column.loading {
            let para = Paragraph::new(Span::styled(
                "loading…",
                Style::default().fg(Color::DarkGray),
            ))
            .block(block);
            f.render_widget(para, rect);
            continue;
        }

        let selected = column.selected;

        // The selected row's label text switches to this color, baked
        // directly into that one row's label span below rather than applied
        // via `highlight_style`'s `fg`, which would patch every cell in the
        // row uniformly (icon included) and overwrite the icon's own color
        // whenever its row is selected.
        let selected_label_fg = if is_focused {
            Color::Rgb(235, 240, 245)
        } else {
            Color::Rgb(210, 212, 216)
        };
        let items: Vec<ListItem> = column
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let label_fg = (Some(i) == selected).then_some(selected_label_fg);
                entry_item(entry, label_fg)
            })
            .collect();

        // No `fg` here (see `selected_label_fg` above) — just the
        // background/weight change, which is fine to patch uniformly since
        // it's not something any span sets a per-entry opinion on.
        let highlight = if is_focused {
            Style::default()
                .bg(Color::Rgb(45, 65, 90))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(Color::Rgb(55, 58, 64))
                .add_modifier(Modifier::BOLD)
        };
        let list = List::new(items).block(block).highlight_style(highlight);

        let col = &mut app.columns[col_idx];
        col.list_state.select(selected);
        f.render_stateful_widget(list, rect, &mut col.list_state);
    }

    draw_preview_column(f, layout.preview, app);
}

fn draw_preview_column(f: &mut Frame, area: Rect, app: &mut App) {
    let title = match app.selected_entry() {
        Some(entry) => entry.name.clone(),
        None => app.cwd(),
    };
    let block = titled_box(app.preview_title_icon(), title, app.preview_focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.preview_viewport_height = inner.height;

    if app.preview_shows_loading() {
        app.preview_viewport_width = inner.width;
        let para = Paragraph::new(Span::styled(
            "loading…",
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(para, inner);
        return;
    }

    let gutter_width = app.preview_gutter_width().min(inner.width);
    let text_width = inner.width - gutter_width;
    app.preview_viewport_width = text_width;

    let text_area = if gutter_width > 0 {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(gutter_width), Constraint::Min(0)])
            .split(inner);
        let gutter = Paragraph::new(Text::from(gutter_lines(app, chunks[0].height, text_width)));
        f.render_widget(gutter, chunks[0]);
        chunks[1]
    } else {
        inner
    };

    let mut para = Paragraph::new(app.preview.clone()).scroll((app.preview_scroll, 0));
    if app.wrap_preview {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, text_area);
}

/// Builds the line-number gutter's content for the visible window of
/// `height` rows starting at `app.preview_scroll`. Numbers always match the
/// preview's real line numbers: with wrapping off this is a plain
/// sequential range, and with wrapping on only the first wrapped row of
/// each source line is numbered — continuation rows are left blank.
fn gutter_lines(app: &App, height: u16, text_width: u16) -> Vec<Line<'static>> {
    let scroll = app.preview_scroll as usize;
    let height = height as usize;
    let total_lines = app.preview.line_count();
    let num_width = app.preview_line_number_width();

    if !app.wrap_preview {
        return (0..height)
            .map(|row| {
                let line_no = scroll + row + 1;
                gutter_line((line_no <= total_lines).then_some(line_no), num_width)
            })
            .collect();
    }

    let text: Text = app.preview.clone().into();
    let mut out = Vec::with_capacity(height);
    let mut virtual_row = 0usize;
    for (idx, line) in text.lines.iter().enumerate() {
        if virtual_row >= scroll + height {
            break;
        }
        let wrapped_rows = Paragraph::new(line.clone())
            .wrap(Wrap { trim: false })
            .line_count(text_width)
            .max(1);
        for sub in 0..wrapped_rows {
            if virtual_row >= scroll + height {
                break;
            }
            if virtual_row >= scroll {
                out.push(gutter_line((sub == 0).then_some(idx + 1), num_width));
            }
            virtual_row += 1;
        }
    }
    while out.len() < height {
        out.push(gutter_line(None, num_width));
    }
    out
}

fn gutter_line(line_no: Option<usize>, num_width: usize) -> Line<'static> {
    let text = match line_no {
        Some(n) => format!("{n:>num_width$} "),
        None => " ".repeat(num_width + 1),
    };
    Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
}

/// Width, in terminal columns, used to indent continuation lines when the
/// footer wraps to more than one row.
const FOOTER_WRAP_INDENT: &str = "        ";

/// Builds the footer content (the main help line, or the toggles line when
/// that menu is open), wrapped to fit `width` columns. Each hint/toggle is
/// treated as a single indivisible "bullet" item: when a line runs out of
/// room, wrapping happens on a bullet boundary rather than mid-item, and
/// continuation lines are indented with `FOOTER_WRAP_INDENT`.
fn footer_text(app: &App, width: u16) -> Text<'static> {
    if app.toggles_menu_open {
        return wrap_items(toggles_footer_items(app), width, FOOTER_WRAP_INDENT, true);
    }

    if let Some(msg) = &app.message {
        return Text::from(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    let gray = Style::default().fg(Color::DarkGray);
    if app.preview_focused {
        let items = vec![
            vec![Span::styled("j/k scroll", gray)],
            vec![Span::styled("h back", gray)],
            vec![Span::styled("gg/G top/bottom", gray)],
            vec![Span::styled("t toggles", gray)],
            vec![Span::styled("q quit", gray)],
        ];
        // No indent here: unlike the column listing, there's nothing on the
        // left of the preview for a wrapped line to visually align under.
        // No header item either, so every gap is a real bullet.
        return wrap_items(items, width, "", false);
    }

    let count = app
        .columns
        .last()
        .map(|column| column.entries.len())
        .unwrap_or(0);
    let items = vec![
        vec![Span::raw(format!("{count} items"))],
        vec![Span::styled("h/j/k/l move", gray)],
        vec![Span::styled("gg/G top/bottom", gray)],
        vec![Span::styled("t toggles", gray)],
        vec![Span::styled("q quit", gray)],
    ];
    wrap_items(items, width, FOOTER_WRAP_INDENT, true)
}

/// Builds the combined toggle list as bullet items: whatever the node
/// source exposes (e.g. "hidden" for a filesystem source) followed by the
/// ambient toggles that apply regardless of source (wrap, line numbers).
fn toggles_footer_items(app: &App) -> Vec<Vec<Span<'static>>> {
    let gray = Style::default().fg(Color::DarkGray);
    let mut items = vec![vec![Span::styled(
        "Toggles",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]];
    for (toggle, on) in &app.source_toggles {
        items.push(vec![toggle_span(toggle.key, &toggle.name, *on)]);
    }
    items.push(vec![toggle_span('w', "wrap", app.wrap_preview)]);
    items.push(vec![toggle_span('n', "numbers", app.show_line_numbers)]);
    items.push(vec![toggle_span('z', "zoom", app.zoom_preview)]);
    items.push(vec![Span::styled("t to exit", gray)]);
    items
}

/// Lays out `items` (each an indivisible bullet) into as few lines as fit
/// within `width`, breaking only between items. The " • " separator is kept
/// between every pair of items, including across a wrap point (it becomes
/// the leading token of the continuation line), so the list still reads as
/// one bulleted sequence. Continuation lines are prefixed with `indent`.
/// When `space_after_first` is set, the first item is treated as a header
/// (e.g. "N items" or "Toggles") and is followed by a plain space instead
/// of a bullet.
fn wrap_items(
    items: Vec<Vec<Span<'static>>>,
    width: u16,
    indent: &'static str,
    space_after_first: bool,
) -> Text<'static> {
    let bullet = || Span::styled(" • ", Style::default().fg(Color::DarkGray));
    let indent_width = indent.chars().count() as u16;

    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width: u16 = 0;

    for (index, item) in items.into_iter().enumerate() {
        let item_width: u16 = item.iter().map(|s| s.content.chars().count() as u16).sum();
        let line_indent = if lines.is_empty() { 0 } else { indent_width };
        let sep = if index == 0 {
            None
        } else if index == 1 && space_after_first {
            Some(Span::raw(" "))
        } else {
            Some(bullet())
        };
        let sep_width = sep.as_ref().map_or(0, |s| s.content.chars().count() as u16);

        if !current.is_empty() && line_indent + current_width + sep_width + item_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if let Some(sep) = sep {
            current_width += sep_width;
            current.push(sep);
        }
        current_width += item_width;
        current.extend(item);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    let rendered: Vec<Line<'static>> = lines
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            if i == 0 || indent.is_empty() {
                Line::from(spans)
            } else {
                let mut with_indent = vec![Span::raw(indent)];
                with_indent.extend(spans);
                Line::from(with_indent)
            }
        })
        .collect();
    Text::from(rendered)
}

fn toggle_span(key: char, label: &str, on: bool) -> Span<'static> {
    let style = if on {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Span::styled(format!("{key} {label}"), style)
}

/// `selected_label_fg`: when this entry is the row under the cursor, the
/// color its label text should switch to (`None` otherwise). Applied only
/// to the label span, deliberately — see the comment where callers compute
/// it in `draw_columns` for why the icon span must stay untouched by
/// selection instead of picking this up via `List::highlight_style`.
fn entry_item(entry: &Entry, selected_label_fg: Option<Color>) -> ListItem<'static> {
    let (label, style) = entry_label(entry);
    let mut spans = Vec::new();
    // `pad_when_missing: true` — unlike a title, this is one row in a
    // column of rows, so an entry with no icon opinion still pads with
    // blank space to keep every row's name starting at the same column.
    // Falls back to the label's own *unselected* color (`style.fg`, not
    // `selected_label_fg`) when the icon has none of its own, so e.g. a
    // folder icon matches the directory-name color it normally sits next
    // to, and keeps that color even while the row is selected.
    if let Some(icon) = nerd_icon_span(
        crate::config::nerd_font(),
        entry.nerd_icon,
        entry.nerd_icon_color,
        style.fg,
        true,
    ) {
        spans.push(icon);
    }
    let label_style = match selected_label_fg {
        Some(fg) => style.fg(fg),
        None => style,
    };
    spans.push(Span::styled(label, label_style));
    ListItem::new(Line::from(spans))
}
