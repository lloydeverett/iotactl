use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::entry::Entry;

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

fn titled_box(title: String, is_focused: bool) -> Block<'static> {
    let (border_style, title_style) = box_style(is_focused);
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Line::from(Span::styled(format!(" {title} "), title_style)))
}

/// Renders the Miller-columns stack of opened directories, followed by a
/// preview column. When there isn't room for every opened column, the
/// oldest (leftmost) ones slide out of view so the focused column and
/// preview stay visible.
fn draw_columns(f: &mut Frame, area: Rect, app: &mut App) {
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

        let items: Vec<ListItem> = column.entries.iter().map(entry_item).collect();
        let selected = column.selected;

        let highlight = if is_focused {
            Style::default()
                .bg(Color::Rgb(45, 65, 90))
                .fg(Color::Rgb(235, 240, 245))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(Color::Rgb(55, 58, 64))
                .fg(Color::Rgb(210, 212, 216))
                .add_modifier(Modifier::BOLD)
        };
        let block = titled_box(app.path_label(&column.id), is_focused);
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
    app.preview_viewport_height = area.height.saturating_sub(2);
    app.preview_viewport_width = area.width.saturating_sub(2);
    let block = titled_box(title, app.preview_focused);
    let mut para = Paragraph::new(app.preview.clone())
        .block(block)
        .scroll((app.preview_scroll, 0));
    if app.wrap_preview {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
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
            "j/k scroll • gg/G top/bottom • w wrap • h back • q quit",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            count_span,
            Span::styled(
                "h/j/k/l move • gg/G top/bottom • H hidden • w wrap • q quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    let para = Paragraph::new(text);
    f.render_widget(para, area);
}

fn entry_item(entry: &Entry) -> ListItem<'static> {
    let (label, style) = if entry.is_dir {
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
    };
    ListItem::new(Span::styled(label, style))
}
