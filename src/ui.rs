use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::entry::Entry;

/// Fixed width of each opened directory column, in terminal cells.
const COLUMN_WIDTH: u16 = 32;
/// Minimum width reserved for the trailing preview column.
const PREVIEW_MIN_WIDTH: u16 = 24;
/// Gap between adjacent columns (and between the last column and the
/// preview column).
const COLUMN_SPACING: u16 = 1;

pub fn draw(f: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(f.area());

    draw_header(f, root[0], app);
    draw_columns(f, root[1], app);
    draw_footer(f, root[2], app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let path = Line::from(vec![Span::styled(
        app.cwd(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(path).block(block);
    f.render_widget(para, area);
}

/// Renders the Miller-columns stack of opened directories, followed by a
/// preview column. When there isn't room for every opened column, the
/// oldest (leftmost) ones slide out of view so the focused column and
/// preview stay visible.
fn draw_columns(f: &mut Frame, area: Rect, app: &mut App) {
    let total = app.columns.len();

    let preview_width = PREVIEW_MIN_WIDTH.min(area.width);
    let available_for_columns = area.width.saturating_sub(preview_width);
    let column_stride = COLUMN_WIDTH + COLUMN_SPACING;
    let max_visible = ((available_for_columns / column_stride).max(1) as usize).min(total);
    let visible = max_visible;
    let start_idx = total - visible;

    let mut constraints: Vec<Constraint> = vec![Constraint::Length(COLUMN_WIDTH); visible];
    constraints.push(Constraint::Min(preview_width));
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .spacing(COLUMN_SPACING)
        .split(area);

    for offset in 0..visible {
        let col_idx = start_idx + offset;
        let is_focused = col_idx == total - 1;
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
        let list = List::new(items).highlight_style(highlight);

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

fn draw_preview_column(f: &mut Frame, area: Rect, app: &App) {
    let para = Paragraph::new(app.preview.clone())
        .block(Block::default())
        .wrap(Wrap { trim: false });
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
    } else {
        Line::from(vec![
            count_span,
            Span::styled(
                "h/j/k/l move • gg/G top/bottom • H hidden • q quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(text).block(block);
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
    } else if entry.is_symlink {
        (
            format!("{}@", entry.name),
            Style::default().fg(Color::Magenta),
        )
    } else {
        (entry.name.clone(), Style::default())
    };
    ListItem::new(Span::styled(label, style))
}
