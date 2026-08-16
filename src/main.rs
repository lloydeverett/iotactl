mod app;
mod command;
mod entry;
mod fs_source;
mod highlight;
mod node_source;
mod sanitize;
mod ui;

use std::env;
use std::io::{self, Stdout};
use std::sync::Arc;

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use app::{App, AppUpdate};
use fs_source::FsSource;

const PAGE_SIZE: i32 = 10;
const HALF_PAGE_SIZE: i32 = PAGE_SIZE / 2;

#[tokio::main]
async fn main() -> io::Result<()> {
    let start_dir = env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("failed to get current directory"))
        .canonicalize()?;

    let root_label = start_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| start_dir.display().to_string());

    let (tx, rx) = mpsc::unbounded_channel::<AppUpdate>();
    let source: Arc<dyn node_source::NodeSource> = Arc::new(FsSource::new(start_dir));
    let app = App::new(Vec::new(), root_label, source, tx).await;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, app, rx).await;
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
    mut updates: mpsc::UnboundedReceiver<AppUpdate>,
) -> io::Result<()> {
    let mut events = EventStream::new();

    loop {
        app.tick();
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let toast_ttl = app.message_ttl();
        let toast_wait = async move {
            match toast_ttl {
                Some(remaining) => tokio::time::sleep(remaining).await,
                None => std::future::pending::<()>().await,
            }
        };

        let preview_loading_ttl = app.preview_loading_ttl();
        let preview_loading_wait = async move {
            match preview_loading_ttl {
                Some(remaining) => tokio::time::sleep(remaining).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut app, key);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e),
                    None => return Ok(()),
                }
            }
            Some(update) = updates.recv() => {
                app.apply_update(update);
            }
            _ = toast_wait => {
                // Toast expired with no input; loop back around to clear it.
            }
            _ = preview_loading_wait => {
                // Debounce window elapsed with the fetch still in flight;
                // loop back around to show the loading placeholder.
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    let was_pending_g = app.pending_g;
    app.pending_g = false;

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.preview_focused {
                app.preview_scroll_by(HALF_PAGE_SIZE)
            } else {
                app.move_selection(HALF_PAGE_SIZE)
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.preview_focused {
                app.preview_scroll_by(-HALF_PAGE_SIZE)
            } else {
                app.move_selection(-HALF_PAGE_SIZE)
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if app.preview_focused {
                app.preview_scroll_by(1)
            } else {
                app.move_selection(1)
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.preview_focused {
                app.preview_scroll_by(-1)
            } else {
                app.move_selection(-1)
            }
        }
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
            if app.preview_focused {
                app.unfocus_preview()
            } else {
                app.go_up()
            }
        }
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if !app.preview_focused {
                app.enter()
            }
        }
        KeyCode::PageDown => {
            if app.preview_focused {
                app.preview_scroll_by(PAGE_SIZE)
            } else {
                app.move_selection(PAGE_SIZE)
            }
        }
        KeyCode::PageUp => {
            if app.preview_focused {
                app.preview_scroll_by(-PAGE_SIZE)
            } else {
                app.move_selection(-PAGE_SIZE)
            }
        }
        KeyCode::Char('g') => {
            if was_pending_g {
                if app.preview_focused {
                    app.preview_scroll_top();
                } else {
                    app.select_first();
                }
            } else {
                app.pending_g = true;
            }
        }
        KeyCode::Char('G') | KeyCode::End => {
            if app.preview_focused {
                app.preview_scroll_bottom()
            } else {
                app.select_last()
            }
        }
        KeyCode::Home => {
            if app.preview_focused {
                app.preview_scroll_top()
            } else {
                app.select_first()
            }
        }
        KeyCode::Char('H') => app.toggle_hidden(),
        KeyCode::Char('w') => app.toggle_wrap(),
        KeyCode::Char('n') => app.toggle_line_numbers(),
        KeyCode::Char('t') => app.toggle_toggles_menu(),
        _ => {}
    }
}
