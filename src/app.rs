use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::text::Text;
use ratatui::widgets::ListState;

use crate::entry::Entry;
use crate::node_source::NodeSource;

/// A single opened directory in the Miller-columns stack.
pub struct Column {
    pub id: Vec<String>,
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
}

pub struct App {
    source: Box<dyn NodeSource>,

    pub show_hidden: bool,

    /// Stack of opened directories, from the fixed start dir (index 0) down
    /// to the currently focused directory (last). Entering a directory
    /// pushes a new column; going up pops the deepest one.
    pub columns: Vec<Column>,
    pub list_state: ListState,

    pub preview: Text<'static>,

    cursor_memory: HashMap<Vec<String>, usize>,
    pub message: Option<String>,
    message_expires_at: Option<Instant>,
    pub should_quit: bool,
    pub pending_g: bool,
}

/// How long an error toast stays on screen before it's cleared.
const TOAST_DURATION: Duration = Duration::from_secs(2);

impl App {
    pub fn new(start: Vec<String>, source: Box<dyn NodeSource>) -> Self {
        let mut app = App {
            source,
            show_hidden: false,
            columns: Vec::new(),
            list_state: ListState::default(),
            preview: Text::default(),
            cursor_memory: HashMap::new(),
            message: None,
            message_expires_at: None,
            should_quit: false,
            pending_g: false,
        };
        let (col, err) = app.load_column(start);
        app.set_message(err);
        app.columns.push(col);
        app.sync_focused_list_state();
        app.update_preview();
        app
    }

    fn load_column(&self, id: Vec<String>) -> (Column, Option<String>) {
        let remembered = self.cursor_memory.get(&id).copied().unwrap_or(0);
        match self.source.read_dir(&id, self.show_hidden) {
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
        format!("/{}", self.focused().id.join("/"))
    }

    fn update_preview(&mut self) {
        self.preview = match self.selected_entry() {
            None => Text::default(),
            Some(entry) => self.source.preview_tui(&entry.id, self.show_hidden),
        };
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
        self.update_preview();
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
        self.update_preview();
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
        self.update_preview();
    }

    /// Opens the selected directory as a new column to the right.
    pub fn enter(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            let (col, err) = self.load_column(entry.id);
            if err.is_none() && col.entries.is_empty() {
                self.set_message(Some(format!("Directory is empty: /{}", col.id.join("/"))));
                return;
            }
            self.columns.push(col);
            self.set_message(err);
            self.sync_focused_list_state();
            self.update_preview();
        } else {
            self.set_message(Some(format!("Not a directory: {}", entry.name)));
        }
    }

    /// Closes the deepest open column. The start directory is a hard
    /// boundary: it can never be closed or navigated above.
    pub fn go_up(&mut self) {
        if self.columns.len() <= 1 {
            return;
        }
        self.columns.pop();
        self.set_message(None);
        self.sync_focused_list_state();
        self.update_preview();
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let ids: Vec<Vec<String>> = self.columns.iter().map(|c| c.id.clone()).collect();
        let mut new_columns = Vec::with_capacity(ids.len());
        let mut last_err = None;
        for id in ids {
            let (col, err) = self.load_column(id);
            if err.is_some() {
                last_err = err;
            }
            new_columns.push(col);
        }
        self.columns = new_columns;
        self.set_message(last_err);
        self.sync_focused_list_state();
        self.update_preview();
    }
}
