use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncSeek};

use crate::command::Command;
use crate::entry::Entry;
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

/// A cheap, cloneable "has this call been superseded?" signal passed into
/// [`NodeSource::preview_tui`]. The caller (`App`) flips it via `cancel()`
/// once a newer request has made the in-flight one moot — e.g. the user
/// moved the selection again before the previous preview finished loading.
///
/// Checking it is purely advisory: `App` still discards a result that
/// arrives after cancellation (by epoch, separately from this flag), so a
/// source is free to ignore `Cancelled` entirely and just run to
/// completion. But for work with a natural per-iteration checkpoint — e.g.
/// scanning a large directory entry by entry — checking `is_cancelled()`
/// there lets the source bail out early instead of finishing an expensive
/// computation whose result was already known to be thrown away. It won't
/// help work that has no such checkpoint, like a single bounded read.
#[derive(Clone, Default)]
pub struct Cancelled(Arc<AtomicBool>);

impl Cancelled {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the call this flag was handed to as superseded.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether `cancel` has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What [`NodeSource::preview_tui`] returns: the styled preview text, plus
/// whatever the source knows about how it should be displayed that the UI
/// couldn't infer on its own.
pub struct Preview {
    pub text: SanitizedText,
    /// Forces the preview pane's line-number gutter off regardless of the
    /// user's line-numbers toggle. Set this when line numbers wouldn't
    /// correspond to anything meaningful in `text` — e.g. a directory
    /// listing or a `key: value` metadata dump rather than file content.
    pub override_disable_line_numbers: bool,
}

impl Preview {
    pub fn new(text: SanitizedText) -> Self {
        Preview {
            text,
            override_disable_line_numbers: false,
        }
    }
}

/// One page of manual content that a node source type contributes,
/// describing itself, to be embedded as a topic in [`crate::manual`]'s page
/// tree. A source implementation (e.g. [`crate::fs::docs`]) publishes these
/// as plain, inert data — it has no knowledge of the manual tree's shape or
/// of `manual` at all. Only [`crate::registry`] (which pairs a source type
/// with its `ManualPage`) and `manual` itself (which splices contributed
/// pages into its tree) know what becomes of them.
pub struct ManualPage {
    /// The id segment this page is reached by, e.g. `"filesystem"`.
    pub slug: &'static str,
    /// Display name, shown as the entry name in a listing.
    pub title: &'static str,
    /// Markdown body shown when this page is a leaf (`children` is empty).
    /// Unused for a category page, whose preview is its child listing
    /// instead.
    pub body: &'static str,
    pub children: &'static [&'static ManualPage],
}

/// A boxed, `Send` handle to a node's raw, unrendered byte content, returned
/// by [`NodeSource::open`]. Unlike [`Preview`] — which trades faithfulness
/// for a bounded, display-ready result (a size limit, binary files skipped,
/// markdown rendered) — this always yields the underlying bytes exactly,
/// however large or unprintable the node is. It's a stream rather than an
/// owned buffer so a caller can consume those bytes incrementally instead of
/// holding the whole thing in memory at once.
pub type ByteStream = Pin<Box<dyn AsyncRead + Send>>;

/// A reader that also supports seeking, so it can be boxed as a single
/// trait object rather than two separate ones over the same underlying
/// value.
pub trait AsyncReadSeek: AsyncRead + AsyncSeek {}
impl<T: AsyncRead + AsyncSeek + ?Sized> AsyncReadSeek for T {}

/// Like [`ByteStream`], but returned by [`NodeSource::open_seekable`]:
/// whenever that call succeeds, the stream it hands back is guaranteed to
/// support seeking, unlike a plain [`ByteStream`] which may or may not.
pub type SeekableByteStream = Pin<Box<dyn AsyncReadSeek + Send>>;

/// Abstracts access to a hierarchy of directories and files.
///
/// # Do real work off the render thread
///
/// `async fn` alone doesn't make a call non-blocking — one that never hits
/// a genuine yield point runs to completion on whatever thread polled it,
/// and that thread is shared with rendering, input handling, and every
/// other in-flight async task (this app runs tokio's multi-threaded
/// runtime). For a directory listing, file read, syntax highlight, or any
/// other real work, hand it to [`tokio::task::spawn_blocking`] and
/// `.await` the `JoinHandle` (mapping a panic to a visible error) instead
/// of running it inline. See [`crate::fs::FsSource::read_dir`]/
/// `preview_tui` and [`crate::manual::ManualSource::preview_tui`] for the
/// pattern — the latter only needs it for one of its two methods, since
/// its `read_dir` is cheap enough that `spawn_blocking` wouldn't be worth
/// it.
///
/// `App` holds up its end too: it dispatches calls via `tokio::spawn` and
/// delivers results back through the `AppUpdate` channel (see
/// `App::dispatch_preview_update_inner`) rather than awaiting a
/// `NodeSource` call directly from a keypress handler or at startup, so a
/// slow fetch never blocks a frame — the UI just shows its normal loading
/// placeholder until the result arrives.
///
/// # Cooperative cancellation
///
/// See [`Cancelled`]'s docs for details: check `is_cancelled()` wherever a
/// natural checkpoint exists (a loop over many entries, a chunked read),
/// but it's fine to skip that for a single bounded operation with nothing
/// to check against.
#[async_trait]
pub trait NodeSource: Send + Sync {
    /// Reads the children of the node at `id`, generally sorted with directories
    /// first, then alphabetically (case-insensitive). Any filtering of the
    /// result (e.g. hidden entries) is entirely up to the source, driven by
    /// whatever toggle state it holds internally.
    ///
    /// See the trait-level docs above if this does anything nontrivial —
    /// it needs the same `spawn_blocking` treatment `preview_tui` does.
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>>;

    /// Entry describing the root node itself (`id == []`). Used for the
    /// initial column's display name and icon, since the root has no
    /// parent entry that could otherwise supply that information — every
    /// other column's title icon is looked up from the `Entry` in its
    /// *parent* column that opened it (see `App::column_icon`), which
    /// doesn't work for the root.
    ///
    /// A source that wants the CLI path to address more finely than just
    /// picking which source to use (e.g. the manual, via `manual://<page>`)
    /// bakes that into its own scope at construction instead — see
    /// `manual::ManualSource`'s `root` field for the pattern — so `id == []`
    /// here still always means "this source's own configured root", never a
    /// deeper node.
    async fn root_entry(&self) -> Entry;

    /// Builds a styled, display-ready preview for the node at `id`. Carries
    /// `SanitizedText` rather than a raw `Text` so every implementation is
    /// forced through the escaping in [`crate::sanitize`] — see that
    /// module's docs for why that matters.
    ///
    /// This is usually where a source's nontrivial work lives (a file read,
    /// a syntax highlight, ...) — see the trait-level docs above for why
    /// that needs `spawn_blocking`, and for what `cancelled` is and when to
    /// check it.
    async fn preview_tui(&self, id: &[String], cancelled: &Cancelled) -> Preview;

    /// Opens the node at `id` for streaming access to its raw, unrendered
    /// bytes: the file's own contents for an `fs` node, the raw markdown
    /// source for a `manual` page, and so on for any other source. Returns a
    /// result in cases `preview_tui` wouldn't — a binary file, or one over
    /// its size limit — since there's no rendering to do and nothing here
    /// needs to fit in memory all at once. Not yet invoked anywhere.
    async fn open(&self, id: &[String]) -> io::Result<ByteStream>;

    /// Like [`open`](NodeSource::open), but guarantees the returned stream
    /// supports seeking whenever this returns `Ok`. A source that can't
    /// offer that guarantee cheaply (e.g. something streamed straight off a
    /// socket, with no way to seek without buffering it all first) should
    /// return an `Unsupported` error here rather than a stream that can't
    /// actually seek. Not yet invoked anywhere.
    async fn open_seekable(&self, id: &[String]) -> io::Result<SeekableByteStream>;

    /// Runs `command` with `args`. Not yet invoked anywhere.
    async fn execute_command(&self, command: &Command, args: &[String]) -> io::Result<()>;
}

/// Describes one kind of node source iotactl can browse, independent of
/// any particular instance of it.
///
/// Toggle get/set live here rather than on [`NodeSource`] because toggle
/// state, for every source type iotactl currently has, is process-global
/// (see e.g. `fs`'s module-level `SHOW_HIDDEN`) — there's only ever one
/// source in use at a time, so there's nothing per-instance to hold. That
/// also means a toggle name can be validated (e.g. a `--toggle-on NAME` at
/// startup — see `crate::app::App::new`) against every registered type at
/// once, via [`crate::registry::toggle_known`], without constructing any of
/// them.
pub struct NodeSourceType {
    /// URI schemes that select this type on the CLI path (e.g.
    /// `"manual://"`), matched by prefix — [`crate::registry::create`]
    /// hands both the matched scheme and the rest of the path to
    /// `construct`. The filesystem type's own scheme (`"file://"`) is
    /// optional: `create` falls back to it, with the path given unaltered
    /// as `rest`, when nothing else matches and the path doesn't otherwise
    /// look like a URI (see `registry`'s private `looks_like_uri`).
    pub schemes: &'static [&'static str],
    /// Manual content this type contributes about itself, embedded as a
    /// top-level topic in the manual's page tree (see [`crate::manual`]).
    /// `None` for a type that doesn't describe itself there — the manual
    /// type itself, whose own pages already are the manual.
    pub manual_page: Option<&'static ManualPage>,
    /// The commands this type of source makes available, independent of
    /// any particular node. Not yet invoked anywhere.
    pub commands: &'static [Command],
    /// The toggles this type of source makes available, independent of
    /// any particular node.
    pub toggles: &'static [Toggle],
    /// Builds this type's source given the scheme that selected it (e.g.
    /// `"file://"`, always non-empty and one of this type's own `schemes`
    /// even when the CLI path itself had none — see
    /// [`crate::registry::create`]'s fallback) and the rest of the CLI path
    /// after that scheme. Handing both pieces over, rather than a single
    /// pre-stripped string, keeps `create` from having to special-case how
    /// much of the path a given type gets to see; it's this function's own
    /// job to decide what to do with them. Any finer addressing within the
    /// source than just picking which type to use (e.g.
    /// `manual://filesystem` picking a page, not just the manual type) is
    /// the source's own job to bake into its scope at construction — see
    /// `ManualSource::new`'s `root` parameter — not something `construct`
    /// reports back out.
    ///
    /// `pipe` is the node source that piped into this one — `Some` when
    /// this segment wasn't the first in a `|`-delimited path (see
    /// [`crate::registry::create`]'s pipe-parsing), `None` otherwise. Only
    /// some types make sense as a pipe's destination (e.g. a future
    /// `zip://`, whose bytes have to come from somewhere); a type that
    /// doesn't should reject a `Some` here rather than silently ignoring
    /// it, and a type that *requires* piping (nothing to browse on its
    /// own) should reject `None` the same way. `fs` and `manual` both
    /// reject `Some`.
    pub(crate) construct_fn:
        fn(&str, &str, Option<Arc<dyn NodeSource>>) -> io::Result<Arc<dyn NodeSource>>,
    /// Sets `toggle` (one of this type's own `toggles`) to `value`.
    pub(crate) set_toggle_fn: fn(&Toggle, bool) -> io::Result<()>,
    /// Reads the current value of `toggle` (one of this type's own
    /// `toggles`).
    pub(crate) get_toggle_fn: fn(&Toggle) -> io::Result<bool>,
}

impl NodeSourceType {
    /// See `construct_fn`'s docs.
    pub fn construct(
        &self,
        scheme: &str,
        rest: &str,
        pipe: Option<Arc<dyn NodeSource>>,
    ) -> io::Result<Arc<dyn NodeSource>> {
        (self.construct_fn)(scheme, rest, pipe)
    }

    /// Whether this type exposes a toggle named `name` — as opposed to
    /// [`crate::registry::toggle_known`], which checks every registered
    /// type at once.
    pub fn has_toggle(&self, name: &str) -> bool {
        self.toggles.iter().any(|t| t.name == name)
    }

    /// See `set_toggle_fn`'s docs.
    pub fn set_toggle(&self, toggle: &Toggle, value: bool) -> io::Result<()> {
        (self.set_toggle_fn)(toggle, value)
    }

    /// See `get_toggle_fn`'s docs.
    pub fn get_toggle(&self, toggle: &Toggle) -> io::Result<bool> {
        (self.get_toggle_fn)(toggle)
    }
}
