use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::entry::Entry;
use crate::entry_preview::{entry_label, nerd_icon_span};

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
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    draw_columns(f, root[0], app);
    draw_footer(f, root[1], app);
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
    nerd_font: bool,
    title: String,
    is_focused: bool,
) -> Block<'static> {
    let (border_style, title_style) = box_style(is_focused);
    let mut spans = vec![Span::styled(" ", title_style)];
    // `pad_when_missing: false` — a title has no column of icons next to it
    // to line up against, so an entry with no icon opinion just omits the
    // span rather than leaving a blank gap.
    if let Some(icon_span) = nerd_icon_span(nerd_font, icon.0, icon.1, None, false) {
        spans.push(icon_span);
    }
    spans.push(Span::styled(format!("{title} "), title_style));
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

    for offset in 0..visible {
        let col_idx = start_idx + offset;
        let is_focused = col_idx == total - 1 && !app.preview_focused;
        let column = &app.columns[col_idx];

        let block = titled_box(
            app.column_icon(col_idx),
            app.nerd_font,
            app.path_label(&column.id),
            is_focused,
        );

        if column.loading {
            let para = Paragraph::new(Span::styled(
                "loading…",
                Style::default().fg(Color::DarkGray),
            ))
            .block(block);
            f.render_widget(para, chunks[offset]);
            continue;
        }

        let selected = column.selected;

        // The selected row's label text switches to this color, same as
        // before — but baked directly into that one row's label span below
        // rather than via `highlight_style`'s `fg`, which patches *every*
        // cell in the row uniformly (icon included) and would otherwise
        // overwrite the icon's own color whenever its row is selected.
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
                entry_item(entry, app.nerd_font, label_fg)
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

        if is_focused {
            app.list_state.select(selected);
            f.render_stateful_widget(list, chunks[offset], &mut app.list_state);
        } else {
            let mut state = ListState::default();
            state.select(selected);
            f.render_stateful_widget(list, chunks[offset], &mut state);
        }
    }

    draw_preview_column(f, chunks[visible], app);
}

fn draw_preview_column(f: &mut Frame, area: Rect, app: &mut App) {
    let title = match app.selected_entry() {
        Some(entry) => app.path_label(&entry.id),
        None => app.cwd(),
    };
    let block = titled_box(
        app.preview_title_icon(),
        app.nerd_font,
        title,
        app.preview_focused,
    );
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

/// Renders the main help line (or the toggles line, when that menu is
/// open). Toggles already have their own listing in `draw_toggles_footer`,
/// so don't add them here even if their key also has a non-toggle use.
fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    if app.toggles_menu_open {
        draw_toggles_footer(f, area, app);
        return;
    }

    let count = app
        .columns
        .last()
        .map(|column| column.entries.len())
        .unwrap_or(0);
    let count_span = Span::raw(format!("{count} items "));

    let text = if let Some(msg) = &app.message {
        Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Red)))
    } else if app.preview_focused {
        Line::from(Span::styled(
            "j/k scroll • h back • gg/G top/bottom • t toggles • q quit",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            count_span,
            Span::styled(
                "h/j/k/l move • gg/G top/bottom • t toggles • q quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    let para = Paragraph::new(text);
    f.render_widget(para, area);
}

/// Renders the combined toggle list: whatever the node source exposes
/// (e.g. "hidden" for a filesystem source) followed by the ambient toggles
/// that apply regardless of source (wrap, line numbers).
fn draw_toggles_footer(f: &mut Frame, area: Rect, app: &App) {
    let separator = || Span::styled(" • ", Style::default().fg(Color::DarkGray));

    let mut spans = vec![Span::styled(
        "Toggles ",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];
    for (toggle, on) in &app.source_toggles {
        spans.push(toggle_span(toggle.key, &toggle.name, *on));
        spans.push(separator());
    }
    spans.push(toggle_span('w', "wrap", app.wrap_preview));
    spans.push(separator());
    spans.push(toggle_span('n', "numbers", app.show_line_numbers));
    spans.push(separator());
    spans.push(toggle_span('z', "zoom", app.zoom_preview));
    spans.push(Span::styled(" • t to exit", Style::default().fg(Color::DarkGray)));

    let para = Paragraph::new(Line::from(spans));
    f.render_widget(para, area);
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
fn entry_item(entry: &Entry, nerd_font: bool, selected_label_fg: Option<Color>) -> ListItem<'static> {
    let (label, style) = entry_label(entry);
    let mut spans = Vec::new();
    // `pad_when_missing: true` — unlike a title, this is one row in a
    // column of rows, so an entry with no icon opinion still pads with
    // blank space to keep every row's name starting at the same column.
    // Falls back to the label's own *unselected* color (`style.fg`, not
    // `selected_label_fg`) when the icon has none of its own, so e.g. a
    // folder icon matches the directory-name color it normally sits next
    // to, and keeps that color even while the row is selected.
    if let Some(icon) = nerd_icon_span(nerd_font, entry.nerd_icon, entry.nerd_icon_color, style.fg, true) {
        spans.push(icon);
    }
    let label_style = match selected_label_fg {
        Some(fg) => style.fg(fg),
        None => style,
    };
    spans.push(Span::styled(label, label_style));
    ListItem::new(Line::from(spans))
}
