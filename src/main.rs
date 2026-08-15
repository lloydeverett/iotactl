mod app;
mod entry;
mod fs_source;
mod node_source;
mod ui;

use std::env;
use std::io::{self, Stdout};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use fs_source::FsSource;

const PAGE_SIZE: i32 = 10;

fn main() -> io::Result<()> {
    let start_dir = env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("failed to get current directory"))
        .canonicalize()?;

    let root_label = start_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| start_dir.display().to_string());

    let mut terminal = setup_terminal()?;
    let result = run(
        &mut terminal,
        App::new(Vec::new(), root_label, Box::new(FsSource::new(start_dir))),
    );
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

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> io::Result<()> {
    loop {
        app.tick();
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if let Some(remaining) = app.message_ttl() {
            if !event::poll(remaining)? {
                // Toast expired with no input; loop back around to clear it.
                continue;
            }
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let was_pending_g = app.pending_g;
        app.pending_g = false;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true
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
            _ => {}
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
