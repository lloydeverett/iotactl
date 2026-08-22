use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::style::Color;
use ratatui::widgets::{ListState, Paragraph, Wrap};
use tokio::sync::mpsc;

use crate::entry::Entry;
use crate::node_source::{Cancelled, NodeSource, NodeSourceType};
use crate::registry;
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
    /// Persisted across frames (unlike a fresh `ListState::default()` per
    /// render) so its `offset()` reflects what was actually last drawn for
    /// this column — needed to map a mouse click's row back to an entry
    /// index. Ratatui computes/writes this during
    /// `render_stateful_widget`, so it's read after render, not derived.
    pub list_state: ListState,
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
    /// The type that `source` is an instance of — used to get/set/validate
    /// toggles, since those now live on `NodeSourceType` rather than on a
    /// source instance (see its docs).
    source_type: &'static NodeSourceType,
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

    /// Stack of opened directories, from the fixed start dir (index 0) down
    /// to the currently focused directory (last). Entering a directory
    /// pushes a new column; going up pops the deepest one.
    pub columns: Vec<Column>,

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
    /// Cancellation flag handed to the in-flight preview fetch, if any.
    /// `dispatch_preview_update_inner` flips it before replacing it with a
    /// fresh one for the new fetch, so a source that checks it (see
    /// `Cancelled`) can stop expensive work early once it's known to be
    /// superseded rather than running to completion for nothing.
    preview_cancel: Option<Cancelled>,
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

/// Fetches every toggle `source_type` exposes along with its current
/// value. Only ever done up front and cached — see `App::source_toggles`
/// — even though, unlike before `NodeSourceType::get_toggle` existed,
/// there'd be nothing wrong with calling it again later; toggle state is
/// process-global now; see `NodeSourceType`'s docs.
fn load_source_toggles(source_type: &'static NodeSourceType) -> Vec<(Toggle, bool)> {
    source_type
        .toggles
        .iter()
        .map(|toggle| (*toggle, source_type.get_toggle(toggle).unwrap_or(false)))
        .collect()
}

/// How long an error toast stays on screen before it's cleared.
const TOAST_DURATION: Duration = Duration::from_secs(2);

/// How long a preview fetch must stay in flight before the loading
/// placeholder is shown. Keeps quick fetches (e.g. a fast tree-sitter
/// parse) from flashing "loading…" between two renders of real content.
const PREVIEW_LOADING_DEBOUNCE: Duration = Duration::from_millis(80);

impl App {
    /// `toggle_overrides` are `(name, value)` pairs applied in order right
    /// after the source's toggles are loaded and before the initial listing
    /// is fetched, so the first render already reflects them — e.g. an
    /// overridden "hidden" toggle affects the very first `read_dir`. A name
    /// that appears more than once (e.g. both `--toggle-on` and
    /// `--toggle-off` for the same toggle, from the CLI and/or
    /// `IOTACTL_FLAGS`) is resolved by whichever pair comes last, since each
    /// is just applied in turn and the last write wins. This only seeds the
    /// initial value; every toggle can still be flipped normally afterwards
    /// from within the TUI. Matches against the ambient toggles ("wrap",
    /// "numbers", "zoom") first, then whatever `source_type` exposes.
    ///
    /// A name that's neither ambient nor exposed by `source_type` doesn't
    /// fail startup by itself: it's only an error if it's unrecognized
    /// *everywhere*, i.e. no known node source type (see
    /// `registry::toggle_known`) exposes a toggle by that name. Otherwise
    /// it's silently ignored — e.g. `--toggle-on meta` while browsing the
    /// manual rather than the filesystem is harmless, since `meta` is a
    /// real toggle, just not one this source has.
    pub async fn new(
        start: Vec<String>,
        source: Arc<dyn NodeSource>,
        source_type: &'static NodeSourceType,
        update_tx: mpsc::UnboundedSender<AppUpdate>,
        toggle_overrides: &[(String, bool)],
    ) -> Self {
        let root_entry = source.root_entry().await;
        let mut source_toggles = load_source_toggles(source_type);
        let mut wrap_preview = false;
        let mut show_line_numbers = false;
        let mut zoom_preview = false;

        for (name, value) in toggle_overrides {
            match name.as_str() {
                "wrap" => wrap_preview = *value,
                "numbers" => show_line_numbers = *value,
                "zoom" => zoom_preview = *value,
                _ => match source_toggles.iter().position(|(t, _)| t.name == name.as_str()) {
                    Some(idx) => {
                        source_toggles[idx].1 = *value;
                        let _ = source_type.set_toggle(&source_toggles[idx].0, *value);
                    }
                    None if !registry::toggle_known(name) => {
                        let mut valid: Vec<&str> = vec!["wrap", "numbers", "zoom"];
                        valid.extend(
                            registry::NODE_SOURCE_TYPES
                                .iter()
                                .flat_map(|t| t.toggles.iter().map(|tg| tg.name)),
                        );
                        valid.sort_unstable();
                        valid.dedup();
                        crate::cli_error::die(format!(
                            "unknown toggle {name:?} (valid toggles: {})",
                            valid.join(", ")
                        ));
                    }
                    None => {}
                },
            }
        }

        let mut app = App {
            source,
            source_type,
            update_tx,
            epoch: 0,
            root_entry,
            source_toggles,
            wrap_preview,
            show_line_numbers,
            zoom_preview,
            columns: Vec::new(),
            preview: SanitizedText::default(),
            preview_override_disable_line_numbers: false,
            preview_loading: false,
            preview_loading_since: None,
            preview_cancel: None,
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

        // The root column's listing is awaited directly rather than going
        // through the dispatch/channel machinery: unlike every other column,
        // it has no parent to fall back to if the load fails or comes back
        // empty (see `fail_column_load`'s doc comment), so `App::new` needs
        // its result up front to build the one column that's always
        // guaranteed to exist. `column_from_result` handles the error case
        // by producing a column with no entries and a toast, rather than by
        // popping a nonexistent parent — see `ColumnLoaded`'s alternative
        // handling in `apply_update` for the contrast.
        //
        // The initial preview is a different story: nothing structural
        // depends on it being ready before the first frame renders, so it's
        // dispatched the same way any other selection change is (see
        // `dispatch_preview_update_immediate`) instead of being awaited
        // here too. That means `App::new` — and so the very first
        // `terminal.draw()` in `main.rs` — never blocks on a source's
        // `preview_tui`, no matter how slow it is; the preview pane just
        // shows its usual loading placeholder until the result arrives over
        // the update channel like any other. See `node_source`'s docs for
        // why a source blocking here at all would be a bug in the source,
        // not just a startup inconvenience.
        let result = app.source.read_dir(&start).await;
        let (col, err) = app.column_from_result(start, result);
        app.set_message(err);
        app.columns.push(col);
        app.sync_focused_list_state();
        app.dispatch_preview_update_immediate();
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
                        list_state: ListState::default(),
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
                        list_state: ListState::default(),
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
        self.focused_mut().list_state.select(selected);
    }

    pub fn cwd(&self) -> String {
        self.column_label(self.columns.len() - 1)
    }

    /// Display name for the column at `idx`, for use as its box's title.
    /// A node's id and its display name can differ (e.g. the manual source
    /// uses a stable slug for the id but a friendlier string, possibly with
    /// spaces or punctuation, as the name) — so, mirroring `column_icon`,
    /// this looks up the `Entry` that was selected in the *previous* column
    /// to open it, rather than assuming the id's last segment doubles as
    /// the name. Falls back to that segment if the entry can no longer be
    /// found (e.g. removed by a concurrent change). The root column
    /// (`idx == 0`) has no such parent, so its label comes from
    /// `root_entry` instead.
    pub fn column_label(&self, idx: usize) -> String {
        let Some(parent) = idx.checked_sub(1).map(|i| &self.columns[i]) else {
            return self.root_entry.name.clone();
        };
        let id = &self.columns[idx].id;
        parent
            .entries
            .iter()
            .find(|e| &e.id == id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| id.last().cloned().unwrap_or_default())
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
            if let Some(cancelled) = self.preview_cancel.take() {
                cancelled.cancel();
            }
            self.preview = SanitizedText::default();
            self.preview_override_disable_line_numbers = false;
            self.preview_loading = false;
            self.preview_loading_since = None;
            self.preview_scroll_restore_percent = None;
            return;
        };

        if let Some(cancelled) = self.preview_cancel.take() {
            cancelled.cancel();
        }
        let cancelled = Cancelled::new();
        self.preview_cancel = Some(cancelled.clone());

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
            let preview = source.preview_tui(&id, &cancelled).await;
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

    /// Updates the focused column's selection and cursor-memory bookkeeping,
    /// without dispatching a preview fetch — callers choose the debounced
    /// or immediate variant afterward depending on whether the column
    /// context just changed (see `dispatch_preview_update` vs. `_immediate`
    /// and `click_entry` below). `idx` must be a valid index into the
    /// focused column's entries.
    fn set_focused_selection(&mut self, idx: usize) {
        let id = self.focused().id.clone();
        let col = self.focused_mut();
        col.selected = Some(idx);
        let name = col.entries[idx].name.clone();
        self.cursor_memory.insert(id, name);
        self.sync_focused_list_state();
    }

    pub fn move_selection(&mut self, delta: i32) {
        let col = self.focused();
        if col.entries.is_empty() {
            return;
        }
        let len = col.entries.len() as i32;
        let current = col.selected.unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len - 1);
        if next == current {
            // Already at the top/bottom of the column: nothing changed, so
            // skip the preview refetch and redraw entirely.
            return;
        }
        self.set_focused_selection(next as usize);
        self.dispatch_preview_update();
    }

    pub fn select_first(&mut self) {
        if self.focused().entries.is_empty() {
            return;
        }
        self.set_focused_selection(0);
        self.dispatch_preview_update();
    }

    pub fn select_last(&mut self) {
        let col = self.focused();
        if col.entries.is_empty() {
            return;
        }
        let last = col.entries.len() - 1;
        self.set_focused_selection(last);
        self.dispatch_preview_update();
    }

    /// Handles a left-click on a row inside the column at `col_idx`
    /// (0-based index into `self.columns`) at `row_idx` (0-based index into
    /// that column's entries). Clicking a row in an earlier, already-open
    /// column truncates the stack back to it first, mirroring pressing `h`
    /// enough times. Clicking the already-selected row in the focused
    /// column opens it, mirroring a second press of `l`/Enter; any other
    /// click just moves the cursor there.
    pub fn click_entry(&mut self, col_idx: usize, row_idx: usize) {
        let Some(column) = self.columns.get(col_idx) else {
            return;
        };
        if row_idx >= column.entries.len() {
            return;
        }

        if col_idx == self.columns.len() - 1 {
            if column.selected == Some(row_idx) {
                self.enter();
            } else {
                self.set_focused_selection(row_idx);
                self.dispatch_preview_update();
            }
            return;
        }

        self.columns.truncate(col_idx + 1);
        self.preview_focused = false;
        self.set_focused_selection(row_idx);
        self.dispatch_preview_update_immediate();
    }

    /// Handles a left-click inside a non-focused column that missed every
    /// row's hitbox (its border/title, or blank space below the last
    /// entry) — moves focus there by truncating the stack back to it, same
    /// as `click_entry`'s column-jump case, but leaves that column's
    /// existing selection untouched since no specific row was clicked.
    pub fn click_column(&mut self, col_idx: usize) {
        if col_idx >= self.columns.len() {
            return;
        }
        self.columns.truncate(col_idx + 1);
        self.preview_focused = false;
        self.dispatch_preview_update_immediate();
    }

    /// Handles a left-click on the preview pane: same as pressing `l`/Enter
    /// on the currently selected entry — opens it as a new column if it's a
    /// directory (the preview is showing that directory's listing), or
    /// focuses the preview pane if it's a file.
    pub fn click_preview(&mut self) {
        self.enter();
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
            list_state: ListState::default(),
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
        let toggle = self.source_toggles[idx].0;
        let _ = self.source_type.set_toggle(&toggle, value);

        self.epoch += 1;
        let epoch = self.epoch;
        let ids: Vec<Vec<String>> = self.columns.iter().map(|c| c.id.clone()).collect();
        let source = Arc::clone(&self.source);
        let tx = self.update_tx.clone();

        tokio::spawn(async move {
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
