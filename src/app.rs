use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::text::Text;
use ratatui::widgets::{ListState, Paragraph, Wrap};
use tokio::sync::mpsc;

use crate::entry::Entry;
use crate::node_source::NodeSource;

/// A single opened directory in the Miller-columns stack.
pub struct Column {
    pub id: Vec<String>,
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
    /// True while this column's listing is still being fetched. Only ever
    /// true for the deepest column, immediately after `enter()` pushes a
    /// placeholder and before its `ColumnLoaded` result arrives.
    pub loading: bool,
}

/// Results of background `NodeSource` calls dispatched by `App`, delivered
/// back to the main loop and applied via `App::apply_update`.
pub enum AppUpdate {
    PreviewLoaded {
        epoch: u64,
        text: Text<'static>,
    },
    ColumnLoaded {
        epoch: u64,
        id: Vec<String>,
        result: io::Result<Vec<Entry>>,
    },
    ColumnsReloaded {
        epoch: u64,
        results: Vec<(Vec<String>, io::Result<Vec<Entry>>)>,
    },
}

pub struct App {
    source: Arc<dyn NodeSource>,
    update_tx: mpsc::UnboundedSender<AppUpdate>,

    /// Bumped before every dispatched background call. Results are tagged
    /// with the epoch captured at dispatch time; `apply_update` discards any
    /// result whose epoch no longer matches, since a newer navigation action
    /// has already made it stale.
    epoch: u64,

    /// Display name for the root node (id == []), e.g. "iotactl", since it
    /// has no path segment of its own to name it. Used as that column's
    /// title.
    root_label: String,

    pub show_hidden: bool,
    /// Whether preview text wraps at the pane width. Off by default so long
    /// lines (e.g. logs, minified files) scroll horizontally-free rather
    /// than reflowing.
    pub wrap_preview: bool,

    /// Stack of opened directories, from the fixed start dir (index 0) down
    /// to the currently focused directory (last). Entering a directory
    /// pushes a new column; going up pops the deepest one.
    pub columns: Vec<Column>,
    pub list_state: ListState,

    pub preview: Text<'static>,
    /// True while a preview fetch is in flight; the preview pane renders a
    /// loading placeholder instead of `preview` while this is set.
    pub preview_loading: bool,
    /// Whether keyboard focus is on the preview pane rather than the
    /// column stack. Only reachable for file entries; directories are
    /// opened as a new column instead.
    pub preview_focused: bool,
    pub preview_scroll: u16,
    /// Height of the preview pane's content area (inside its border), as
    /// measured by the last render. Used to clamp/compute scroll offsets.
    pub preview_viewport_height: u16,
    /// Width of the preview pane's content area (inside its border), as
    /// measured by the last render. Used alongside `wrap_preview` to work
    /// out how many rows the text actually renders to once wrapped, since
    /// wrapped lines can outnumber raw text lines.
    pub preview_viewport_width: u16,

    cursor_memory: HashMap<Vec<String>, usize>,
    pub message: Option<String>,
    message_expires_at: Option<Instant>,
    pub should_quit: bool,
    pub pending_g: bool,
}

/// How long an error toast stays on screen before it's cleared.
const TOAST_DURATION: Duration = Duration::from_secs(2);

impl App {
    pub async fn new(
        start: Vec<String>,
        root_label: String,
        source: Arc<dyn NodeSource>,
        update_tx: mpsc::UnboundedSender<AppUpdate>,
    ) -> Self {
        let mut app = App {
            source,
            update_tx,
            epoch: 0,
            root_label,
            show_hidden: false,
            wrap_preview: false,
            columns: Vec::new(),
            list_state: ListState::default(),
            preview: Text::default(),
            preview_loading: false,
            preview_focused: false,
            preview_scroll: 0,
            preview_viewport_height: 0,
            preview_viewport_width: 0,
            cursor_memory: HashMap::new(),
            message: None,
            message_expires_at: None,
            should_quit: false,
            pending_g: false,
        };

        // Nothing else is running yet, so the initial load can be awaited
        // directly instead of going through the dispatch/channel machinery.
        let show_hidden = app.show_hidden;
        let result = app.source.read_dir(&start, show_hidden).await;
        let (col, err) = app.column_from_result(start, result);
        app.set_message(err);
        app.columns.push(col);
        app.sync_focused_list_state();
        if let Some(entry) = app.selected_entry() {
            app.preview = app.source.preview_tui(&entry.id, show_hidden).await;
        }
        app
    }

    /// Turns a `read_dir` result into a `Column` plus an optional error
    /// toast message, applying the remembered cursor position.
    fn column_from_result(
        &self,
        id: Vec<String>,
        result: io::Result<Vec<Entry>>,
    ) -> (Column, Option<String>) {
        let remembered = self.cursor_memory.get(&id).copied().unwrap_or(0);
        match result {
            Ok(entries) => {
                let selected = if entries.is_empty() {
                    None
                } else {
                    Some(remembered.min(entries.len() - 1))
                };
                (
                    Column {
                        id,
                        entries,
                        selected,
                        loading: false,
                    },
                    None,
                )
            }
            Err(e) => {
                let msg = format!("Error reading /{}: {e}", id.join("/"));
                (
                    Column {
                        id,
                        entries: Vec::new(),
                        selected: None,
                        loading: false,
                    },
                    Some(msg),
                )
            }
        }
    }

    fn set_message(&mut self, msg: Option<String>) {
        self.message_expires_at = msg.as_ref().map(|_| Instant::now() + TOAST_DURATION);
        self.message = msg;
    }

    /// Time remaining before the message toast should be cleared, if one is
    /// showing. The caller can use this to wake up exactly when needed
    /// instead of polling on a fixed interval.
    pub fn message_ttl(&self) -> Option<Duration> {
        self.message_expires_at
            .map(|expires_at| expires_at.saturating_duration_since(Instant::now()))
    }

    /// Clears the message toast once its display time has elapsed. Called
    /// on every event loop tick so the toast disappears on its own.
    pub fn tick(&mut self) {
        if let Some(expires_at) = self.message_expires_at {
            if Instant::now() >= expires_at {
                self.message = None;
                self.message_expires_at = None;
            }
        }
    }

    fn focused(&self) -> &Column {
        self.columns.last().expect("columns is never empty")
    }

    fn focused_mut(&mut self) -> &mut Column {
        self.columns.last_mut().expect("columns is never empty")
    }

    fn sync_focused_list_state(&mut self) {
        let selected = self.focused().selected;
        self.list_state.select(selected);
    }

    pub fn cwd(&self) -> String {
        self.path_label(&self.focused().id)
    }

    /// Display name for a node id: just the node's own name, e.g.
    /// `path_label(&["a", "b"])` is `"b"`. The root (id == []) has no name
    /// of its own, so it falls back to `root_label`. Used as the title of
    /// each column's and the preview's box.
    pub fn path_label(&self, id: &[String]) -> String {
        match id.last() {
            Some(name) => name.clone(),
            None => self.root_label.clone(),
        }
    }

    /// Dispatches a background fetch of the preview for the currently
    /// selected entry, showing a loading placeholder until it arrives. If
    /// nothing is selected, clears the preview immediately with no fetch.
    fn dispatch_preview_update(&mut self) {
        self.preview_scroll = 0;
        let Some(id) = self.selected_entry().map(|entry| entry.id.clone()) else {
            self.preview = Text::default();
            self.preview_loading = false;
            return;
        };

        self.epoch += 1;
        let epoch = self.epoch;
        let show_hidden = self.show_hidden;
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();
        self.preview_loading = true;

        tokio::spawn(async move {
            let text = source.preview_tui(&id, show_hidden).await;
            let _ = tx.send(AppUpdate::PreviewLoaded { epoch, text });
        });
    }

    fn preview_max_scroll(&self) -> u16 {
        let total_lines = if self.wrap_preview {
            let para = Paragraph::new(self.preview.clone()).wrap(Wrap { trim: false });
            para.line_count(self.preview_viewport_width) as u16
        } else {
            self.preview.lines.len() as u16
        };
        total_lines.saturating_sub(self.preview_viewport_height)
    }

    /// Moves keyboard focus onto the preview pane. Only meaningful for file
    /// entries: directories are opened as a new column via `enter` instead.
    pub fn focus_preview(&mut self) {
        if matches!(self.selected_entry(), Some(entry) if !entry.is_dir) {
            self.preview_focused = true;
            self.preview_scroll = 0;
        }
    }

    pub fn unfocus_preview(&mut self) {
        self.preview_focused = false;
    }

    pub fn preview_scroll_by(&mut self, delta: i32) {
        let max = self.preview_max_scroll() as i32;
        let current = self.preview_scroll as i32;
        self.preview_scroll = (current + delta).clamp(0, max) as u16;
    }

    pub fn preview_scroll_top(&mut self) {
        self.preview_scroll = 0;
    }

    pub fn preview_scroll_bottom(&mut self) {
        self.preview_scroll = self.preview_max_scroll();
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        let col = self.focused();
        col.selected.and_then(|i| col.entries.get(i))
    }

    pub fn move_selection(&mut self, delta: i32) {
        let id = self.focused().id.clone();
        let col = self.focused_mut();
        if col.entries.is_empty() {
            return;
        }
        let len = col.entries.len() as i32;
        let current = col.selected.unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len - 1) as usize;
        col.selected = Some(next);
        self.cursor_memory.insert(id, next);
        self.sync_focused_list_state();
        self.dispatch_preview_update();
    }

    pub fn select_first(&mut self) {
        let id = self.focused().id.clone();
        let col = self.focused_mut();
        if col.entries.is_empty() {
            return;
        }
        col.selected = Some(0);
        self.cursor_memory.insert(id, 0);
        self.sync_focused_list_state();
        self.dispatch_preview_update();
    }

    pub fn select_last(&mut self) {
        let id = self.focused().id.clone();
        let col = self.focused_mut();
        if col.entries.is_empty() {
            return;
        }
        let last = col.entries.len() - 1;
        col.selected = Some(last);
        self.cursor_memory.insert(id, last);
        self.sync_focused_list_state();
        self.dispatch_preview_update();
    }

    /// Opens the selected directory as a new column to the right. Pushes a
    /// loading placeholder immediately and dispatches the real fetch in the
    /// background; `apply_update` resolves it once the listing arrives.
    pub fn enter(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if !entry.is_dir {
            self.focus_preview();
            return;
        }

        self.columns.push(Column {
            id: entry.id.clone(),
            entries: Vec::new(),
            selected: None,
            loading: true,
        });
        self.preview_focused = false;
        self.sync_focused_list_state();

        self.epoch += 1;
        let epoch = self.epoch;
        let id = entry.id;
        let show_hidden = self.show_hidden;
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();

        tokio::spawn(async move {
            let result = source.read_dir(&id, show_hidden).await;
            let _ = tx.send(AppUpdate::ColumnLoaded { epoch, id, result });
        });
    }

    /// Closes the deepest open column. The start directory is a hard
    /// boundary: it can never be closed or navigated above.
    pub fn go_up(&mut self) {
        if self.columns.len() <= 1 {
            return;
        }
        self.columns.pop();
        self.preview_focused = false;
        self.set_message(None);
        self.sync_focused_list_state();
        self.dispatch_preview_update();
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap_preview = !self.wrap_preview;
        self.preview_scroll = self.preview_scroll.min(self.preview_max_scroll());
    }

    /// Reloads every currently-open column with the new `show_hidden`
    /// setting, applying the whole stack atomically once every column's
    /// listing has arrived (so the UI never shows a half-reloaded stack).
    pub fn toggle_hidden(&mut self) {
        self.preview_focused = false;
        self.show_hidden = !self.show_hidden;

        self.epoch += 1;
        let epoch = self.epoch;
        let ids: Vec<Vec<String>> = self.columns.iter().map(|c| c.id.clone()).collect();
        let show_hidden = self.show_hidden;
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();

        tokio::spawn(async move {
            let mut results = Vec::with_capacity(ids.len());
            for id in ids {
                let result = source.read_dir(&id, show_hidden).await;
                results.push((id, result));
            }
            let _ = tx.send(AppUpdate::ColumnsReloaded { epoch, results });
        });
    }

    /// Applies a background fetch result to app state. Results tagged with a
    /// stale epoch (superseded by a newer navigation action) are discarded.
    pub fn apply_update(&mut self, update: AppUpdate) {
        match update {
            AppUpdate::PreviewLoaded { epoch, text } => {
                if epoch != self.epoch {
                    return;
                }
                self.preview = text;
                self.preview_loading = false;
            }
            AppUpdate::ColumnLoaded { epoch, id, result } => {
                if epoch != self.epoch {
                    return;
                }
                match result {
                    Ok(entries) if entries.is_empty() => {
                        self.columns.pop();
                        self.preview_focused = false;
                        self.sync_focused_list_state();
                        self.set_message(Some(format!(
                            "Directory is empty: /{}",
                            id.join("/")
                        )));
                    }
                    Ok(entries) => {
                        let remembered = self.cursor_memory.get(&id).copied().unwrap_or(0);
                        let selected = Some(remembered.min(entries.len() - 1));
                        if let Some(col) = self.columns.last_mut() {
                            col.entries = entries;
                            col.selected = selected;
                            col.loading = false;
                        }
                        self.sync_focused_list_state();
                        self.dispatch_preview_update();
                    }
                    Err(e) => {
                        if let Some(col) = self.columns.last_mut() {
                            col.loading = false;
                        }
                        self.set_message(Some(format!("Error reading /{}: {e}", id.join("/"))));
                        self.sync_focused_list_state();
                        self.dispatch_preview_update();
                    }
                }
            }
            AppUpdate::ColumnsReloaded { epoch, results } => {
                if epoch != self.epoch {
                    return;
                }
                let mut new_columns = Vec::with_capacity(results.len());
                let mut last_err = None;
                for (id, result) in results {
                    let (col, err) = self.column_from_result(id, result);
                    if err.is_some() {
                        last_err = err;
                    }
                    new_columns.push(col);
                }
                self.columns = new_columns;
                self.set_message(last_err);
                self.sync_focused_list_state();
                self.dispatch_preview_update();
            }
        }
    }
}
