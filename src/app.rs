use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::style::Color;
use ratatui::widgets::{ListState, Paragraph, Wrap};
use tokio::sync::mpsc;

use crate::entry::Entry;
use crate::node_source::NodeSource;
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

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
        text: SanitizedText,
        override_disable_line_numbers: bool,
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

    /// The root node (id == []) itself, obtained from the source via
    /// `NodeSource::root_entry`. Since the root has no path segment of its
    /// own, its `name` is used as the initial column's title, and its icon
    /// fields as that column's title icon (see `column_icon`).
    root_entry: Entry,

    /// Toggles supplied by the node source (e.g. "hidden" for a filesystem
    /// source), paired with their current values. The source is the sole
    /// owner of what each one means and does; this is just a cache of its
    /// state, kept because rendering the footer is synchronous while
    /// `NodeSource::get_toggle` is not. Combined with the ambient,
    /// always-applicable toggles (`wrap_preview`, `show_line_numbers`) when
    /// the footer is drawn.
    pub source_toggles: Vec<(Toggle, bool)>,
    /// Whether preview text wraps at the pane width. Off by default so long
    /// lines (e.g. logs, minified files) scroll horizontally-free rather
    /// than reflowing.
    pub wrap_preview: bool,
    /// Whether the preview pane shows a line-number gutter.
    pub show_line_numbers: bool,
    /// Whether the preview pane expands to fill the whole screen while it's
    /// focused, hiding the column stack. Has no visible effect while the
    /// column stack is focused instead — see `draw_columns`.
    pub zoom_preview: bool,
    /// Whether Nerd Font icons are drawn to the left of filenames (in
    /// listings and window titles). Set once from the `--nerd-font` CLI
    /// flag and never changed at runtime.
    pub nerd_font: bool,

    /// Stack of opened directories, from the fixed start dir (index 0) down
    /// to the currently focused directory (last). Entering a directory
    /// pushes a new column; going up pops the deepest one.
    pub columns: Vec<Column>,
    pub list_state: ListState,

    pub preview: SanitizedText,
    /// Set from the most recently loaded preview's
    /// `Preview::override_disable_line_numbers` — forces the gutter off
    /// regardless of `show_line_numbers` when the current preview isn't
    /// line-numbered content (a directory listing, a metadata dump). See
    /// `preview_gutter_width`.
    pub preview_override_disable_line_numbers: bool,
    /// True while a preview fetch is in flight. The preview pane keeps
    /// showing the previous `preview` until `PREVIEW_LOADING_DEBOUNCE` has
    /// elapsed (see `preview_shows_loading`), so a fast fetch never flashes
    /// the loading placeholder.
    pub preview_loading: bool,
    /// When the in-flight preview fetch started, if any. Used to debounce
    /// the loading placeholder.
    preview_loading_since: Option<Instant>,
    /// Whether keyboard focus is on the preview pane rather than the
    /// column stack. Only reachable for file entries; directories are
    /// opened as a new column instead.
    pub preview_focused: bool,
    pub preview_scroll: u16,
    /// Set by `dispatch_preview_update_preserving_scroll` just before
    /// dispatch, as `preview_scroll`'s fraction of the *old* preview's max
    /// scroll. Consumed by `apply_update` once the new preview arrives to
    /// re-derive `preview_scroll` as that same fraction of the *new*
    /// preview's max scroll — since a toggle like "raw" can change the
    /// line count, the same absolute offset would land somewhere else.
    preview_scroll_restore_percent: Option<f64>,
    /// Height of the preview pane's content area (inside its border), as
    /// measured by the last render. Used to clamp/compute scroll offsets.
    pub preview_viewport_height: u16,
    /// Width of the preview pane's content area (inside its border), as
    /// measured by the last render. Used alongside `wrap_preview` to work
    /// out how many rows the text actually renders to once wrapped, since
    /// wrapped lines can outnumber raw text lines.
    pub preview_viewport_width: u16,

    /// Remembers the *name* (not index) of the last-selected entry in each
    /// column, so the cursor can be re-found after a reload that shuffles
    /// indices — e.g. toggling hidden files changes which index a given
    /// entry sits at.
    cursor_memory: HashMap<Vec<String>, String>,
    pub message: Option<String>,
    message_expires_at: Option<Instant>,
    pub should_quit: bool,
    pub pending_g: bool,
    /// Whether the toggles menu is open. While open, the status bar shows
    /// each toggle's state and key instead of the usual help text.
    pub toggles_menu_open: bool,
}

/// Fetches every toggle the source exposes along with its current value.
/// `NodeSource::get_toggle` is async since a real implementation may need
/// I/O, so this is only ever done up front and cached — see
/// `App::source_toggles`.
async fn load_source_toggles(source: &Arc<dyn NodeSource>) -> Vec<(Toggle, bool)> {
    let mut toggles = Vec::new();
    for toggle in source.available_toggles().iter() {
        let value = source.get_toggle(toggle).await.unwrap_or(false);
        toggles.push((toggle.clone(), value));
    }
    toggles
}

/// How long an error toast stays on screen before it's cleared.
const TOAST_DURATION: Duration = Duration::from_secs(2);

/// How long a preview fetch must stay in flight before the loading
/// placeholder is shown. Keeps quick fetches (e.g. a fast tree-sitter
/// parse) from flashing "loading…" between two renders of real content.
const PREVIEW_LOADING_DEBOUNCE: Duration = Duration::from_millis(80);

impl App {
    pub async fn new(
        start: Vec<String>,
        source: Arc<dyn NodeSource>,
        update_tx: mpsc::UnboundedSender<AppUpdate>,
        nerd_font: bool,
    ) -> Self {
        let root_entry = source.root_entry().await;
        let source_toggles = load_source_toggles(&source).await;
        let mut app = App {
            source,
            update_tx,
            epoch: 0,
            root_entry,
            source_toggles,
            wrap_preview: false,
            show_line_numbers: false,
            zoom_preview: false,
            nerd_font,
            columns: Vec::new(),
            list_state: ListState::default(),
            preview: SanitizedText::default(),
            preview_override_disable_line_numbers: false,
            preview_loading: false,
            preview_loading_since: None,
            preview_focused: false,
            preview_scroll: 0,
            preview_scroll_restore_percent: None,
            preview_viewport_height: 0,
            preview_viewport_width: 0,
            cursor_memory: HashMap::new(),
            message: None,
            message_expires_at: None,
            should_quit: false,
            pending_g: false,
            toggles_menu_open: false,
        };

        // Nothing else is running yet, so the initial load can be awaited
        // directly instead of going through the dispatch/channel machinery.
        let result = app.source.read_dir(&start).await;
        let (col, err) = app.column_from_result(start, result);
        app.set_message(err);
        app.columns.push(col);
        app.sync_focused_list_state();
        if let Some(entry) = app.selected_entry() {
            let preview = app.source.preview_tui(&entry.id).await;
            app.preview = preview.text;
            app.preview_override_disable_line_numbers = preview.override_disable_line_numbers;
        }
        app
    }

    /// Resolves the remembered selection for column `id` against a freshly
    /// loaded `entries` list. Looks the entry up by name first, since a
    /// reload (e.g. toggling hidden files) can shuffle indices; falls back
    /// to index 0 if the remembered entry is no longer present (e.g. it was
    /// a hidden file and hidden files were just turned off).
    fn resolve_selection(&self, id: &[String], entries: &[Entry]) -> usize {
        match self.cursor_memory.get(id) {
            Some(name) => entries.iter().position(|e| &e.name == name).unwrap_or(0),
            None => 0,
        }
    }

    /// Turns a `read_dir` result into a `Column` plus an optional error
    /// toast message, applying the remembered cursor position.
    fn column_from_result(
        &self,
        id: Vec<String>,
        result: io::Result<Vec<Entry>>,
    ) -> (Column, Option<String>) {
        match result {
            Ok(entries) => {
                let selected = if entries.is_empty() {
                    None
                } else {
                    Some(self.resolve_selection(&id, &entries))
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
            None => self.root_entry.name.clone(),
        }
    }

    /// Nerd Font icon/color for the column at `idx`'s title, looked up from
    /// the `Entry` that was selected in the *previous* column to open it —
    /// since a `Column` only stores an `id`, not the `Entry` that produced
    /// it. The root column (`idx == 0`) has no such parent, so its icon
    /// comes from `root_entry` (obtained from the source itself) instead;
    /// same fallback if the entry can no longer be found in a non-root
    /// parent (e.g. it was removed by a concurrent change on disk).
    pub fn column_icon(&self, idx: usize) -> (Option<char>, Option<Color>) {
        let Some(parent) = idx.checked_sub(1).map(|i| &self.columns[i]) else {
            return (self.root_entry.nerd_icon, self.root_entry.nerd_icon_color);
        };
        parent
            .entries
            .iter()
            .find(|e| e.id == self.columns[idx].id)
            .map(|e| (e.nerd_icon, e.nerd_icon_color))
            .unwrap_or((None, None))
    }

    /// Nerd Font icon/color for the preview pane's title: the selected
    /// entry's, or no icon when nothing is selected (the pane falls back to
    /// showing `cwd()` as its title in that case, which isn't an `Entry`).
    pub fn preview_title_icon(&self) -> (Option<char>, Option<Color>) {
        self.selected_entry()
            .map(|e| (e.nerd_icon, e.nerd_icon_color))
            .unwrap_or((None, None))
    }

    /// Dispatches a background fetch of the preview for the currently
    /// selected entry, showing a loading placeholder until it arrives. If
    /// nothing is selected, clears the preview immediately with no fetch.
    ///
    /// The old preview is held on screen for `PREVIEW_LOADING_DEBOUNCE`
    /// before the loading placeholder replaces it, so a fast fetch never
    /// flashes "loading…" between two renders of real content. That only
    /// makes sense when the old and new previews are of the same kind of
    /// thing — e.g. moving the cursor between files in one directory. Going
    /// up a level (see `go_up`) changes context entirely, so the held-over
    /// preview would itself be a wrong-content flash; callers there should
    /// use `dispatch_preview_update_immediate` instead.
    fn dispatch_preview_update(&mut self) {
        self.dispatch_preview_update_inner(true, false);
    }

    /// Like `dispatch_preview_update`, but skips the debounce so the loading
    /// placeholder (or the new preview, if the fetch is fast) appears right
    /// away instead of holding over the previous preview.
    fn dispatch_preview_update_immediate(&mut self) {
        self.dispatch_preview_update_inner(false, false);
    }

    /// Like `dispatch_preview_update`, but for refetching the preview of the
    /// entry that's *already* selected — e.g. after a source toggle changed
    /// (see `toggle_source_toggle`) — rather than a new selection. Captures
    /// the current scroll position as a fraction of the current preview's
    /// max scroll, so `apply_update` can restore the same fraction once the
    /// new preview (which may have a different line count, e.g. "raw" mode
    /// showing/hiding markdown markers) arrives, instead of snapping to the
    /// top. Only meaningful when the selected node itself hasn't changed —
    /// callers should fall back to `dispatch_preview_update` otherwise.
    fn dispatch_preview_update_preserving_scroll(&mut self) {
        let max_scroll = self.preview_max_scroll();
        self.preview_scroll_restore_percent = Some(if max_scroll == 0 {
            0.0
        } else {
            (self.preview_scroll as f64 / max_scroll as f64).clamp(0.0, 1.0)
        });
        self.dispatch_preview_update_inner(true, true);
    }

    fn dispatch_preview_update_inner(&mut self, debounce: bool, preserve_scroll: bool) {
        if !preserve_scroll {
            self.preview_scroll = 0;
            self.preview_scroll_restore_percent = None;
        }
        let Some(id) = self.selected_entry().map(|entry| entry.id.clone()) else {
            self.preview = SanitizedText::default();
            self.preview_override_disable_line_numbers = false;
            self.preview_loading = false;
            self.preview_loading_since = None;
            self.preview_scroll_restore_percent = None;
            return;
        };

        self.epoch += 1;
        let epoch = self.epoch;
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();
        self.preview_loading = true;
        self.preview_loading_since = Some(if debounce {
            Instant::now()
        } else {
            Instant::now() - PREVIEW_LOADING_DEBOUNCE
        });

        tokio::spawn(async move {
            let preview = source.preview_tui(&id).await;
            let _ = tx.send(AppUpdate::PreviewLoaded {
                epoch,
                text: preview.text,
                override_disable_line_numbers: preview.override_disable_line_numbers,
            });
        });
    }

    /// Whether the preview pane should show the loading placeholder rather
    /// than the previous `preview`. False for the first
    /// `PREVIEW_LOADING_DEBOUNCE` after a fetch starts, so a fetch that
    /// completes quickly never displaces the old preview at all.
    pub fn preview_shows_loading(&self) -> bool {
        match self.preview_loading_since {
            Some(since) => since.elapsed() >= PREVIEW_LOADING_DEBOUNCE,
            None => false,
        }
    }

    /// Time remaining before the loading placeholder should appear, if a
    /// preview fetch is in flight and still within the debounce window.
    /// The caller can use this to wake up exactly when needed instead of
    /// polling on a fixed interval.
    pub fn preview_loading_ttl(&self) -> Option<Duration> {
        let since = self.preview_loading_since?;
        Some(PREVIEW_LOADING_DEBOUNCE.saturating_sub(since.elapsed()))
    }

    fn preview_max_scroll(&self) -> u16 {
        let total_lines = if self.wrap_preview {
            let para = Paragraph::new(self.preview.clone()).wrap(Wrap { trim: false });
            para.line_count(self.preview_viewport_width) as u16
        } else {
            self.preview.line_count() as u16
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
        if next as i32 == current {
            // Already at the top/bottom of the column: nothing changed, so
            // skip the preview refetch and redraw entirely.
            return;
        }
        col.selected = Some(next);
        let name = col.entries[next].name.clone();
        self.cursor_memory.insert(id, name);
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
        let name = col.entries[0].name.clone();
        self.cursor_memory.insert(id, name);
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
        let name = col.entries[last].name.clone();
        self.cursor_memory.insert(id, name);
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
        // The new column has no selection yet, so this just clears the
        // preview instead of dispatching a fetch. Without it, the pane
        // would keep showing the preview of the directory we just entered
        // — now the wrong context — until `ColumnLoaded` arrives; see
        // `go_up`'s use of the immediate variant for the same reason.
        self.dispatch_preview_update_immediate();

        self.epoch += 1;
        let epoch = self.epoch;
        let id = entry.id;
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();

        tokio::spawn(async move {
            let result = source.read_dir(&id).await;
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
        self.dispatch_preview_update_immediate();
    }

    /// Abandons the deepest column after its `read_dir` came back unusable
    /// (a real error, or — per `apply_update`'s `ColumnLoaded` handling — an
    /// empty listing, which isn't worth opening as a column of its own).
    /// Pops back to the parent, shows `message` as a toast, and — since
    /// `enter()` cleared the preview immediately for the column being
    /// abandoned (see `dispatch_preview_update_immediate`'s doc comment) —
    /// redispatches it so the pane reflects the re-selected entry (e.g. an
    /// "empty directory" placeholder) instead of staying blank.
    fn fail_column_load(&mut self, message: String) {
        self.columns.pop();
        self.preview_focused = false;
        self.sync_focused_list_state();
        self.set_message(Some(message));
        self.dispatch_preview_update_immediate();
    }

    pub fn toggle_toggles_menu(&mut self) {
        self.toggles_menu_open = !self.toggles_menu_open;
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap_preview = !self.wrap_preview;
        self.preview_scroll = self.preview_scroll.min(self.preview_max_scroll());
    }

    pub fn toggle_line_numbers(&mut self) {
        self.show_line_numbers = !self.show_line_numbers;
        self.preview_scroll = self.preview_scroll.min(self.preview_max_scroll());
    }

    pub fn toggle_zoom(&mut self) {
        self.zoom_preview = !self.zoom_preview;
        self.preview_scroll = self.preview_scroll.min(self.preview_max_scroll());
    }

    /// Digit width used to format line numbers in the preview gutter: wide
    /// enough for the highest line number, but never less than 3.
    pub fn preview_line_number_width(&self) -> usize {
        self.preview.line_count().max(1).to_string().len().max(3)
    }

    /// Width of the preview pane's line-number gutter, including its
    /// trailing space separator; zero when the toggle is off, or when the
    /// current preview's source has overridden line numbers off (e.g. a
    /// directory listing or metadata dump rather than file content).
    pub fn preview_gutter_width(&self) -> u16 {
        if !self.show_line_numbers || self.preview_override_disable_line_numbers {
            return 0;
        }
        self.preview_line_number_width() as u16 + 1
    }

    /// Flips whichever source toggle is bound to `key` (a no-op if the
    /// current source doesn't expose one bound to it) and reloads every
    /// currently-open column, applying the whole stack atomically once
    /// every column's listing has arrived (so the UI never shows a
    /// half-reloaded stack). Any source toggle can in principle change what
    /// `read_dir` returns — "hidden" is just the one source toggle that
    /// exists today — so this reload dance is generic rather than specific
    /// to it. Callers outside the source (main's key handling, the footer)
    /// never need to know what the toggle is called or does: they learn of
    /// its existence and its key purely by querying `source_toggles`.
    pub fn toggle_source_toggle(&mut self, key: char) {
        let Some(idx) = self.source_toggles.iter().position(|(t, _)| t.key == key) else {
            return;
        };
        let value = !self.source_toggles[idx].1;
        self.source_toggles[idx].1 = value;
        let toggle = self.source_toggles[idx].0.clone();

        self.epoch += 1;
        let epoch = self.epoch;
        let ids: Vec<Vec<String>> = self.columns.iter().map(|c| c.id.clone()).collect();
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();

        tokio::spawn(async move {
            let _ = source.set_toggle(&toggle, value).await;
            let mut results = Vec::with_capacity(ids.len());
            for id in ids {
                let result = source.read_dir(&id).await;
                results.push((id, result));
            }
            let _ = tx.send(AppUpdate::ColumnsReloaded { epoch, results });
        });
    }

    /// Applies a background fetch result to app state. Results tagged with a
    /// stale epoch (superseded by a newer navigation action) are discarded.
    pub fn apply_update(&mut self, update: AppUpdate) {
        match update {
            AppUpdate::PreviewLoaded {
                epoch,
                text,
                override_disable_line_numbers,
            } => {
                if epoch != self.epoch {
                    return;
                }
                self.preview = text;
                self.preview_override_disable_line_numbers = override_disable_line_numbers;
                self.preview_loading = false;
                self.preview_loading_since = None;
                if let Some(percent) = self.preview_scroll_restore_percent.take() {
                    let max_scroll = self.preview_max_scroll();
                    self.preview_scroll = (percent * max_scroll as f64).round() as u16;
                    self.preview_scroll = self.preview_scroll.min(max_scroll);
                }
            }
            AppUpdate::ColumnLoaded { epoch, id, result } => {
                if epoch != self.epoch {
                    return;
                }
                match result {
                    Ok(entries) if entries.is_empty() => {
                        self.fail_column_load(format!("Directory is empty: /{}", id.join("/")));
                    }
                    Ok(entries) => {
                        let selected = Some(self.resolve_selection(&id, &entries));
                        if let Some(col) = self.columns.last_mut() {
                            col.entries = entries;
                            col.selected = selected;
                            col.loading = false;
                        }
                        self.sync_focused_list_state();
                        self.dispatch_preview_update();
                    }
                    Err(e) => {
                        self.fail_column_load(format!("Error reading /{}: {e}", id.join("/")));
                    }
                }
            }
            AppUpdate::ColumnsReloaded { epoch, results } => {
                if epoch != self.epoch {
                    return;
                }
                // Remember the previously selected entry so we only unfocus
                // the preview pane (as `enter`/`go_up` do) when the reload
                // actually moved the cursor onto a different entry — e.g.
                // toggling hidden files while the cursor sits on an
                // always-visible one. The preview itself is always refetched
                // below regardless: a source toggle is a black box from
                // here, and one like "raw" changes `preview_tui`'s output
                // for the very entry that's still selected, so the fetch
                // can't be skipped just because the selection didn't move.
                let previous_selection = self.selected_entry().map(|e| e.id.clone());

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

                let same_selection =
                    self.selected_entry().map(|e| &e.id) == previous_selection.as_ref();
                if same_selection {
                    // Same node, so keep the scroll position — as a
                    // fraction of the (possibly now-different) content
                    // length — rather than snapping back to the top.
                    self.dispatch_preview_update_preserving_scroll();
                } else {
                    self.preview_focused = false;
                    self.dispatch_preview_update();
                }
            }
        }
    }
}
