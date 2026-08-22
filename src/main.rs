mod app;
mod cli_error;
mod command;
mod config;
mod entry;
mod entry_preview;
mod fs;
mod highlight;
mod json;
mod manual;
mod node_source;
mod registry;
mod sanitize;
mod streams;
mod toggle;
mod ui;
mod zip;

use std::env;
use std::io::{self, Stdout};

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use app::{App, AppUpdate};
use cli_error::die;

const PAGE_SIZE: i32 = 10;
const HALF_PAGE_SIZE: i32 = PAGE_SIZE / 2;

/// Default for `--slow-pipe-buffer-size`.
const DEFAULT_SLOW_PIPE_BUFFER_SIZE: usize = 8 * 1024 * 1024;

/// Parses a byte count for `--slow-pipe-buffer-size`: a plain integer, or
/// one followed by a `k`/`m`/`g` (or `kb`/`mb`/`gb`) suffix, case-insensitive,
/// each multiplying by 1024 (matching e.g. `dd`'s `bs=` and `ls -h`, rather
/// than `k`/`m`/`g` meaning a decimal 1000).
fn parse_byte_size(s: &str) -> Result<usize, String> {
    let lower = s.trim().to_ascii_lowercase();
    let (digits, multiplier) =
        if let Some(n) = lower.strip_suffix("kb").or_else(|| lower.strip_suffix('k')) {
            (n, 1024)
        } else if let Some(n) = lower.strip_suffix("mb").or_else(|| lower.strip_suffix('m')) {
            (n, 1024 * 1024)
        } else if let Some(n) = lower.strip_suffix("gb").or_else(|| lower.strip_suffix('g')) {
            (n, 1024 * 1024 * 1024)
        } else if let Some(n) = lower.strip_suffix('b') {
            (n, 1)
        } else {
            (lower.as_str(), 1)
        };
    let count: usize = digits.trim().parse().map_err(|_| {
        format!("{s:?} is not a valid size: expected a number, optionally followed by k, m, or g")
    })?;
    count
        .checked_mul(multiplier)
        .ok_or_else(|| format!("{s:?} is too large"))
}

#[derive(Parser)]
#[command(
    version,
    args_override_self = true,
    after_help = "Run `iotactl manual://` to open the built-in manual."
)]
struct Cli {
    /// Directory to start browsing from. Defaults to the current directory.
    /// May be given as file://<path> explicitly. Pass manual:// instead to
    /// open the built-in manual, optionally followed by a topic id (e.g.
    /// manual://filesystem) to open it there instead of at the top level.
    path: Option<String>,

    /// Show Nerd Font icons to the left of filenames, in listings and
    /// window titles. Requires a terminal font patched with Nerd Fonts.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_nerd_font")]
    nerd_font: bool,

    /// Disable Nerd Font icons (default).
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "nerd_font")]
    no_nerd_font: bool,

    /// Enable mouse support: click to select/open, scroll to navigate (default).
    #[arg(long, action = ArgAction::SetTrue, default_value_t = true, overrides_with = "no_mouse")]
    mouse: bool,

    /// Disable mouse support.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "mouse")]
    no_mouse: bool,

    /// Set a toggle on at startup (repeatable). See the 't' toggles menu for
    /// valid names.
    #[arg(long = "toggle-on", value_name = "NAME")]
    toggle_on: Vec<String>,

    /// Set a toggle off at startup (repeatable). See --toggle-on.
    #[arg(long = "toggle-off", value_name = "NAME")]
    toggle_off: Vec<String>,

    /// Node source implementations characteristically avoid streaming large
    /// buffers into memory, but this is impossible to avoid in the general
    /// case for certain pipe configurations (e.g. `file://large.zip |
    /// zip://big.zip | zip://`) without incurring computational cost (e.g.
    /// due to repeated and frequent stream decompressions in the example).
    /// Turning on this flag opts in to that cost.
    #[arg(long, action = ArgAction::SetTrue)]
    allow_slow_pipes: bool,

    /// Size of the buffer simulated seeking (see --allow-slow-pipes) keeps
    /// of the most recently streamed bytes. Takes a plain number of bytes,
    /// or one followed by k, m, or g (kb/mb/gb also accepted; case doesn't
    /// matter), e.g. 8m. A seek backward that lands within this buffer
    /// replays from it instead of restarting the stream from scratch, so
    /// this lets us avoid repeated re-streaming if the data fits inside
    /// this memory buffer. A larger buffer holds more per open stream
    /// (memory cost) and replays bytes as they were when first streamed
    /// rather than re-reading them, so it can go stale if the underlying
    /// pipe's data changes concurrently. 0 disables the buffer.
    #[arg(long, value_name = "SIZE", value_parser = parse_byte_size, default_value_t = DEFAULT_SLOW_PIPE_BUFFER_SIZE)]
    slow_pipe_buffer_size: usize,
}

/// Arg ids (the derived `Cli` field names) allowed to be set via
/// `IOTACTL_FLAGS`. A new arg is unsettable from the environment by default
/// — add its id here only when that's deliberately wanted. `path` is
/// deliberately never on this list: a positional smuggled in through the
/// environment would otherwise let it silently redirect which directory
/// gets opened.
const ENV_ALLOWED_ARGS: &[&str] = &[
    "nerd_font",
    "no_nerd_font",
    "mouse",
    "no_mouse",
    "toggle_on",
    "toggle_off",
    "allow_slow_pipes",
    "slow_pipe_buffer_size",
];

/// Shell-word-splits `IOTACTL_FLAGS`, if set and non-empty.
fn env_flag_args() -> Option<Vec<String>> {
    let raw = env::var("IOTACTL_FLAGS").ok().filter(|s| !s.trim().is_empty())?;
    Some(shlex::split(&raw).unwrap_or_else(|| {
        die("failed to parse IOTACTL_FLAGS environment variable")
    }))
}

/// Rejects `env_args` if they'd set anything outside [`ENV_ALLOWED_ARGS`].
/// Parses them in isolation (as if they were the whole invocation) purely to
/// see which arg ids they set — the real parse of the merged argv happens
/// separately in `parse_cli`.
fn check_env_args_allowed(env_args: &[String]) {
    // `disable_help_flag`/`disable_version_flag` make `--help`/`--version`
    // behave like any other unrecognized flag here (clap's usual "unexpected
    // argument" error) instead of immediately printing and exiting — those
    // aren't on `ENV_ALLOWED_ARGS` either, so they should be rejected the
    // same way, not given a free pass just because clap injects them.
    let cmd = Cli::command()
        .disable_help_flag(true)
        .disable_version_flag(true);
    let known_ids: Vec<String> = cmd
        .get_arguments()
        .map(|a| a.get_id().as_str().to_string())
        .collect();
    let matches = cmd
        .no_binary_name(true)
        .try_get_matches_from(env_args)
        .unwrap_or_else(|e| e.exit());
    for id in &known_ids {
        // `value_source` distinguishes an arg actually supplied on this
        // (env-only) command line from one merely holding its default —
        // `matches.ids()` includes both, which would otherwise flag e.g.
        // `mouse`'s `default_value_t` as if the environment had set it.
        if matches.value_source(id.as_str()) != Some(clap::parser::ValueSource::CommandLine) {
            continue;
        }
        if !ENV_ALLOWED_ARGS.contains(&id.as_str()) {
            let display = if id == "path" {
                "the PATH argument".to_string()
            } else {
                format!("--{}", id.replace('_', "-"))
            };
            die(format!("IOTACTL_FLAGS may not set {display}"));
        }
    }
}

/// Builds the effective argv by splicing `IOTACTL_FLAGS` (shell-word split,
/// and checked against [`ENV_ALLOWED_ARGS`]) in ahead of the real
/// command-line arguments. Placing them first means an explicit
/// command-line flag takes precedence over the same flag from the
/// environment, since clap keeps the last occurrence of a given option.
fn build_args() -> Vec<String> {
    let mut argv: Vec<String> = env::args().collect();
    let Some(env_args) = env_flag_args() else {
        return argv;
    };
    check_env_args_allowed(&env_args);
    let rest = argv.split_off(1);
    argv.extend(env_args);
    argv.extend(rest);
    argv
}

/// Parses the effective argv (see [`build_args`]) into a [`Cli`], plus the
/// `--toggle-on`/`--toggle-off` pairs in the order they actually appeared.
/// `Cli`'s derived `Vec<String>` fields can't express that interleaving on
/// their own (each only records occurrences of its own flag), so this goes
/// through `ArgMatches::indices_of` — which assigns every value a position
/// in the flat argument list — to recover it, rather than hand-rolling a
/// second argument parser that would have to duplicate clap's `--flag=value`
/// handling to stay in sync with it.
fn parse_cli() -> (Cli, Vec<(String, bool)>) {
    let matches = Cli::command().get_matches_from(build_args());
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    let mut overrides: Vec<(usize, String, bool)> = Vec::new();
    if let (Some(indices), Some(values)) = (
        matches.indices_of("toggle_on"),
        matches.get_many::<String>("toggle_on"),
    ) {
        overrides.extend(indices.zip(values).map(|(i, v)| (i, v.clone(), true)));
    }
    if let (Some(indices), Some(values)) = (
        matches.indices_of("toggle_off"),
        matches.get_many::<String>("toggle_off"),
    ) {
        overrides.extend(indices.zip(values).map(|(i, v)| (i, v.clone(), false)));
    }
    overrides.sort_by_key(|(i, ..)| *i);

    let toggle_overrides = overrides.into_iter().map(|(_, name, on)| (name, on)).collect();
    (cli, toggle_overrides)
}

#[tokio::main]
async fn main() {
    if let Err(e) = run_iotactl().await {
        die(e);
    }
}

/// Runs the program end to end. Kept separate from `main` so that any
/// `io::Error` bubbling out — including ones just handed a friendlier
/// message below — goes through [`die`] and prints as a plain `error: ...`
/// line instead of `main`'s default Debug-formatted dump.
async fn run_iotactl() -> io::Result<()> {
    let (cli, toggle_overrides) = parse_cli();

    let path_arg = cli.path.unwrap_or_else(|| ".".to_string());

    config::init(
        cli.nerd_font && !cli.no_nerd_font,
        cli.allow_slow_pipes,
        cli.slow_pipe_buffer_size,
    );
    let mouse = cli.mouse && !cli.no_mouse;

    let (tx, rx) = mpsc::unbounded_channel::<AppUpdate>();
    let (source, source_type) = registry::create(&path_arg).await?;
    let app = App::new(Vec::new(), source, source_type, tx, &toggle_overrides).await;

    let mut terminal = setup_terminal(mouse)?;
    let result = run(&mut terminal, app, rx).await;
    restore_terminal(&mut terminal, mouse)?;
    result
}

fn setup_terminal(mouse: bool) -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mouse: bool) -> io::Result<()> {
    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
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
                    Some(Ok(Event::Mouse(mouse))) => {
                        handle_mouse(&mut app, mouse, terminal.size()?);
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
        KeyCode::Char('w') => app.toggle_wrap(),
        KeyCode::Char('n') => app.toggle_line_numbers(),
        KeyCode::Char('z') => app.toggle_zoom(),
        KeyCode::Char('t') => app.toggle_toggles_menu(),
        // Any other character key may be bound to a toggle the node source
        // exposes (e.g. "hidden" for a filesystem source) — main.rs has no
        // knowledge of what those are or what they're called, only that
        // `App` knows how to look one up by key.
        KeyCode::Char(c) => app.toggle_source_toggle(c),
        _ => {}
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent, term_size: ratatui::layout::Size) {
    let term_area = ratatui::layout::Rect::new(0, 0, term_size.width, term_size.height);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            match ui::hit_test(term_area, app, mouse.column, mouse.row) {
                Some(ui::MouseTarget::Entry { col_idx, row_idx }) => {
                    app.click_entry(col_idx, row_idx)
                }
                Some(ui::MouseTarget::Column { col_idx }) => app.click_column(col_idx),
                Some(ui::MouseTarget::Preview) => app.click_preview(),
                None => {}
            }
        }
        MouseEventKind::ScrollDown => {
            if ui::point_in_preview(term_area, app, mouse.column, mouse.row) {
                app.preview_scroll_by(1)
            } else {
                app.move_selection(1)
            }
        }
        MouseEventKind::ScrollUp => {
            if ui::point_in_preview(term_area, app, mouse.column, mouse.row) {
                app.preview_scroll_by(-1)
            } else {
                app.move_selection(-1)
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_byte_size;

    #[test]
    fn parse_byte_size_accepts_plain_bytes() {
        assert_eq!(parse_byte_size("0"), Ok(0));
        assert_eq!(parse_byte_size("1024"), Ok(1024));
        assert_eq!(parse_byte_size(" 1024 "), Ok(1024));
    }

    #[test]
    fn parse_byte_size_accepts_binary_suffixes_case_insensitively() {
        for suffix in ["k", "K", "kb", "KB", "Kb"] {
            assert_eq!(parse_byte_size(&format!("2{suffix}")), Ok(2 * 1024));
        }
        for suffix in ["m", "M", "mb", "MB"] {
            assert_eq!(parse_byte_size(&format!("2{suffix}")), Ok(2 * 1024 * 1024));
        }
        for suffix in ["g", "G", "gb", "GB"] {
            assert_eq!(
                parse_byte_size(&format!("2{suffix}")),
                Ok(2 * 1024 * 1024 * 1024)
            );
        }
        assert_eq!(parse_byte_size("512b"), Ok(512));
    }

    #[test]
    fn parse_byte_size_rejects_garbage() {
        assert!(parse_byte_size("8x").is_err());
        assert!(parse_byte_size("kb").is_err());
        assert!(parse_byte_size("").is_err());
    }
}
