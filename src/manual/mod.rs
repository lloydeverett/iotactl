//! A `NodeSource` that shows the iotactl manual instead of any real data.
//! Reached by giving `manual://` as the path on the command line.
//!
//! The manual is a fixed tree of pages, baked into this binary as markdown
//! text. A page with children is a category: selecting it previews the
//! list of its children, same as a directory. A page with no children is a
//! leaf: selecting it previews its own markdown text, rendered through
//! [`crate::highlight::highlighted_text`] — the same code `fs` uses for a
//! real `.md` file — so this page's own `raw` toggle (see
//! [`crate::highlight::RAW_TOGGLE_NAME`]) shows or hides its markdown marks
//! exactly like a markdown file preview does.
//!
//! This module owns only the tree structure and the pages that describe
//! iotactl itself (e.g. navigation). A page that documents another node
//! source type (e.g. the filesystem source) is instead contributed by that
//! type itself and spliced into the tree via [`crate::registry`] — see
//! [`crate::node_source::NodeSourceType::manual_page`] — so this module never
//! needs to know that other source types even exist, let alone how they
//! work.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use ratatui::style::{Color, Style};

use crate::command::Command;
use crate::entry::Entry;
use crate::entry_preview;
use crate::highlight;
use crate::node_source::{Cancelled, ManualPage, NodeSource, NodeSourceType, Preview};
use crate::registry;
use crate::sanitize::SanitizedText;
use crate::streams::{ByteStream, SeekableByteStream};
use crate::toggle::Toggle;

/// Backing state for [`highlight::RAW_TOGGLE_NAME`], the manual's only
/// toggle. Process-global rather than a field on `ManualSource` since only
/// one source is ever in use at a time — see [`NodeSourceType`]'s docs for
/// why that lets get/set live on the type instead of on a source instance.
static RAW_MARKDOWN: AtomicBool = AtomicBool::new(false);

/// This type's contribution to [`crate::registry::NODE_SOURCE_TYPES`].
pub static NODE_SOURCE_TYPE: NodeSourceType = NodeSourceType {
    // A prefix, not just an exact match: anything after it addresses a
    // page within the manual (e.g. `manual://filesystem`), split on `/`
    // into an id — the same shape `find_page` walks for ordinary in-app
    // navigation. Empty for bare `manual://`, meaning "start at the top".
    schemes: &["manual://"],
    manual_page: None,
    commands: &[],
    toggles: &[Toggle {
        name: highlight::RAW_TOGGLE_NAME,
        key: highlight::RAW_TOGGLE_KEY,
    }],
    construct_fn: |_scheme, rest, pipe| {
        let rest = rest.to_string();
        Box::pin(async move {
            if pipe.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "manual:// can't be piped into from another node source",
                ));
            }
            Ok(Arc::new(ManualSource::new(&rest)?) as Arc<dyn NodeSource>)
        })
    },
    set_toggle_fn: |toggle, value| {
        if toggle.name == highlight::RAW_TOGGLE_NAME {
            RAW_MARKDOWN.store(value, Ordering::SeqCst);
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the manual has no toggle named {:?}", toggle.name),
            ))
        }
    },
    get_toggle_fn: |toggle| {
        if toggle.name == highlight::RAW_TOGGLE_NAME {
            Ok(RAW_MARKDOWN.load(Ordering::SeqCst))
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the manual has no toggle named {:?}", toggle.name),
            ))
        }
    },
};

/// A synthetic filename handed to [`highlight::highlighted_text`] purely so
/// it infers "markdown" as the language — every manual page's `body` is
/// markdown, so this never varies per page. No file by this name is ever
/// actually opened.
const MANUAL_PAGE_PATH: &str = "manual.md";

/// Fixed top-level topics about iotactl and the manual itself. Every node
/// source type may contribute one more top-level topic of its own — see
/// `ROOT` below — but these four describe iotactl itself rather than any
/// particular source, so they aren't sourced from anywhere else.
static FIXED_ROOT_CHILDREN: &[&ManualPage] = &[
    &ManualPage {
        slug: "about-manual",
        title: "About This Manual",
        body: ABOUT_MANUAL,
        children: &[],
    },
    &ManualPage {
        slug: "cli",
        title: "Command-Line Options",
        body: CLI_OPTIONS,
        children: &[],
    },
    &ManualPage {
        slug: "navigation",
        title: "Navigation & Keybindings",
        body: NAVIGATION,
        children: &[],
    },
    &ManualPage {
        slug: "node-sources",
        title: "Node Sources",
        body: NODE_SOURCES_OVERVIEW,
        children: &[],
    },
];

/// The manual's page tree: `FIXED_ROOT_CHILDREN` above, plus one more
/// top-level topic per node source type that has something to say about
/// itself (see `crate::node_source::NodeSourceType::manual_page`). Built
/// lazily, once, on first access — a plain `static` can't splice a
/// registry-provided list into a compile-time array the way this does.
static ROOT: LazyLock<ManualPage> = LazyLock::new(|| {
    let mut children: Vec<&'static ManualPage> = FIXED_ROOT_CHILDREN.to_vec();
    children.extend(
        registry::NODE_SOURCE_TYPES
            .iter()
            .filter_map(|source_type| source_type.manual_page),
    );
    ManualPage {
        slug: "",
        title: "iotactl Manual",
        body: "",
        children: Box::leak(children.into_boxed_slice()),
    }
});

/// Written in ASD-STE100 style (short sentences, one idea per sentence,
/// plain words) since this text is user-facing.
const ABOUT_MANUAL: &str = "\
# About This Manual

This is the manual for iotactl. It is a node source, like the filesystem source.

## Structure

The manual has a tree of topics. A topic with sub-topics is a category. Open a
category to see its sub-topics. A topic with no sub-topics is a page. Open a
page to read its text.

## Markdown and the raw toggle

Every page in this manual is written in markdown. Markdown text uses marks such
as `#` for a heading and `*` for emphasis.

By default, iotactl hides these marks and shows styled text instead. Press `r`
to show the raw markdown text, marks and all. Press `r` again to hide the marks.

This is the same raw toggle the filesystem source uses for markdown files. See
the \"Filesystem\" topic's \"Toggles\" page.

## Opening the manual

Run `iotactl manual://` to open this manual instead of a real directory.";

const CLI_OPTIONS: &str = "\
# Command-Line Options

Run `iotactl [OPTIONS] [PATH]`.

## PATH

PATH sets where iotactl starts browsing.

- Give a real path to browse it as a directory.
- Give no path to browse the current directory.
- Give `file://` followed by a real path to browse it explicitly. Same as
  giving the path alone, but useful if the path itself could be mistaken for
  something else.
- Give `manual://` to open this manual instead.
- Give `manual://` followed by a topic's id (e.g. `manual://filesystem`) to
  open the manual there instead of at its top level.

## Nerd Font icons

- `--nerd-font` shows an icon next to each entry. Your terminal font must
  support Nerd Fonts.
- `--no-nerd-font` hides icons. This is the default.

## Mouse support

- `--mouse` turns on mouse support. This is the default. Click an entry to open
  it. Scroll to move the list.
- `--no-mouse` turns off mouse support.

## Toggles at startup

- `--toggle-on NAME` turns a named toggle on at startup. Use this flag more than
  once to set more than one toggle.
- `--toggle-off NAME` turns a named toggle off at startup.

See a source's own topic in this manual for its toggle names.

## The IOTACTL_FLAGS environment variable

Set IOTACTL_FLAGS to a string of flags. iotactl applies these flags before your
command-line flags, so a command-line flag always wins over the environment
variable.

Only some flags are allowed in IOTACTL_FLAGS: `--nerd-font`, `--no-nerd-font`,
`--mouse`, `--no-mouse`, `--toggle-on`, and `--toggle-off`. iotactl rejects any
other flag, and rejects a PATH value, in IOTACTL_FLAGS.";

const NAVIGATION: &str = "\
# Navigation & Keybindings

## Moving the cursor

- `j` or Down: move down.
- `k` or Up: move up.
- `h`, Left, or Backspace: go up one level.
- `l`, Right, or Enter: open the selected entry.

## Jumping

- Press `g` twice: jump to the first entry.
- `G` or End: jump to the last entry.
- Home: jump to the first entry.

## Paging

- Page Up or Page Down: move by one page.
- Ctrl+u or Ctrl+d: move by half a page.

## The preview pane

- `w`: turn word wrap on or off.
- `n`: turn line numbers on or off.
- `z`: zoom the preview pane to fill the screen.

## Toggles

- `t`: open the toggles menu.
- Any other key may be bound to a toggle. See a source's own topic in this
  manual for its toggle names and keys.

## Mouse

- Click an entry to select it.
- Click a column to focus it.
- Scroll to move the list, or to scroll the preview.

## Quitting

- `q`, Esc, or Ctrl+C: quit iotactl.";

const NODE_SOURCES_OVERVIEW: &str = "\
# Node Sources

A node source is a tree that iotactl can browse. Each node in the tree has:

- A name, shown in listings.
- An id, used to find it again.
- Child nodes, if it is a category or directory.
- Preview content, shown when you select it.

## Why this design

iotactl does not assume every tree is a set of real files. Each source decides
what its nodes mean, and how to read them.

## Sources in this build

- Filesystem: real files and directories on disk. See the \"Filesystem\" topic.
- Manual: this manual. See the topics at the top level of this tree.

Only one source is active at a time, chosen by the PATH argument you pass to
iotactl. See the \"Command-Line Options\" topic.";

/// Walks `id` down from the root, one segment per level, matching each
/// segment against a child's `slug` (case-insensitively, so e.g. a
/// CLI-supplied `manual://Filesystem` reaches the same page a lowercase
/// `manual://filesystem` would — every id `find_page` sees that *isn't*
/// CLI-supplied is already exactly the stored slug, so this never changes
/// which page ordinary in-app navigation reaches). Returns `None` if any
/// segment doesn't match — the same "no such node" case a real source would
/// report if a caller handed it a stale or invalid id.
fn find_page(id: &[String]) -> Option<&'static ManualPage> {
    let mut page: &'static ManualPage = &ROOT;
    for segment in id {
        page = page
            .children
            .iter()
            .find(|child| child.slug.eq_ignore_ascii_case(segment))
            .copied()?;
    }
    Some(page)
}

fn no_such_page(id: &[String]) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no such manual page: {}", id.join("/")),
    )
}

fn error_text(msg: String) -> SanitizedText {
    SanitizedText::from_text(&msg, Style::default().fg(Color::Red))
}

/// Icon for a manual entry, per the same directory-vs-document convention
/// `fs` uses: a category (has children) gets the shared folder glyph with
/// no color opinion of its own (see `entry_preview::FOLDER_ICON`); a leaf
/// page is markdown, so it gets the shared markdown glyph/color (see
/// `highlight::MARKDOWN_ICON`) that `fs` also uses for a real `.md` file.
fn entry_icon(is_dir: bool) -> (char, Option<Color>) {
    if is_dir {
        (entry_preview::FOLDER_ICON, None)
    } else {
        let (icon, color) = highlight::MARKDOWN_ICON;
        (icon, Some(color))
    }
}

/// Builds the `Entry` for `page`, addressed at `id` — shared by
/// `child_entries` (one call per child of the page being listed) and
/// `ManualSource::root_entry` (one call for whatever page `App`'s `start`
/// points at).
fn page_entry(id: Vec<String>, page: &ManualPage) -> Entry {
    let is_dir = !page.children.is_empty();
    let (icon, icon_color) = entry_icon(is_dir);
    Entry {
        name: page.title.to_string(),
        id,
        is_dir,
        is_link: false,
        suggested_commands: Arc::from(Vec::new()),
        nerd_icon: Some(icon),
        nerd_icon_color: icon_color,
    }
}

fn child_entries(id: &[String], page: &ManualPage) -> Vec<Entry> {
    page.children
        .iter()
        .map(|child| {
            let mut child_id = id.to_vec();
            child_id.push(child.slug.to_string());
            page_entry(child_id, child)
        })
        .collect()
}

/// Splits the CLI path's rest after `manual://` into id segments — the same
/// shape `NodeSource::read_dir` takes. Ignores empty segments (a leading,
/// trailing, or doubled `/`) rather than erroring, so e.g.
/// `manual://filesystem/` behaves the same as `manual://filesystem`.
fn split_id(rest: &str) -> Vec<String> {
    rest.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A `NodeSource` that shows the fixed manual tree defined in this module,
/// scoped at construction to `root` — same idea as `fs::FsSource` scoping
/// itself to a root directory, just with a fixed in-memory tree standing in
/// for the filesystem. Every id this source is given via the `NodeSource`
/// trait (e.g. `App`'s `start`, always `[]`) is relative to `root`, not to
/// this module's true top level — see `absolute`.
pub struct ManualSource {
    /// Absolute id (within this module's fixed page tree — see
    /// `find_page`) that this source is scoped to. `[]` for the plain
    /// `manual://` case (scoped at the tree's own top level); e.g.
    /// `["filesystem"]` for `manual://filesystem`.
    root: Vec<String>,
}

impl ManualSource {
    /// `root` (the CLI path's rest after `manual://`, e.g. `filesystem` in
    /// `manual://filesystem`) is split into id segments and then scopes this
    /// source the way `fs::FsSource::new`'s `root` parameter scopes a
    /// filesystem source: every id given to this source afterward is
    /// resolved relative to it (see `absolute`). Rejected eagerly, before
    /// any part of the app is built, if it doesn't name a real page —
    /// mirroring `FsSource::new` rejecting a nonexistent directory the same
    /// way.
    pub fn new(root: &str) -> io::Result<Self> {
        let root = split_id(root);
        find_page(&root).ok_or_else(|| no_such_page(&root))?;
        Ok(ManualSource { root })
    }

    /// Resolves a relative `id` (as given to any `NodeSource` method) to
    /// the absolute id `find_page` expects.
    fn absolute(&self, id: &[String]) -> Vec<String> {
        self.root.iter().chain(id).cloned().collect()
    }
}

#[async_trait]
impl NodeSource for ManualSource {
    // Not `spawn_blocking`'d, unlike `fs::FsSource::read_dir` — this only
    // ever walks this module's small, fixed, in-memory page tree, cheap
    // enough that dispatching it to the blocking thread pool would cost
    // more than it saves. See `preview_tui`'s leaf-page branch below for
    // the case in this source that *does* need it, and `node_source`'s
    // docs for the general rule.
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let absolute = self.absolute(id);
        let page = find_page(&absolute).ok_or_else(|| no_such_page(&absolute))?;
        Ok(child_entries(id, page))
    }

    async fn root_entry(&self) -> Entry {
        // Always succeeds: `self.root` was already validated by
        // `ManualSource::new`, and never changes afterward.
        let page = find_page(&self.root).expect("ManualSource::new validated `root`");
        page_entry(Vec::new(), page)
    }

    async fn preview_tui(&self, id: &[String], _cancelled: &Cancelled) -> Preview {
        let absolute = self.absolute(id);
        let Some(page) = find_page(&absolute) else {
            return Preview::new(error_text(no_such_page(&absolute).to_string()));
        };
        if page.children.is_empty() {
            // Reconciled with `fs::FsSource::preview_tui`: syntax-highlighting
            // a page's markdown is real CPU work — parsing, and on a cache
            // miss, building that language's `HighlightConfiguration` (see
            // `highlight::get_config`) — so, like `fs`, it must not run
            // inline on the async runtime thread. See `node_source`'s docs
            // for why. `body`/`hide_markers` are both cheap `Copy` values
            // (a `&'static str` and a `bool`), so there's no need to clone
            // `self` into the closure the way `fs` clones its whole source —
            // only the two values the blocking work actually needs move in.
            let hide_markers = !RAW_MARKDOWN.load(Ordering::SeqCst);
            let body = page.body;
            tokio::task::spawn_blocking(move || {
                Preview::new(highlight::highlighted_text(
                    Path::new(MANUAL_PAGE_PATH),
                    body,
                    hide_markers,
                ))
            })
            .await
            .unwrap_or_else(|_| Preview::new(error_text("panicked while loading preview".to_string())))
        } else {
            // Unlike the branch above, this only ever walks this module's
            // small, fixed, in-memory page tree (see `child_entries`) — far
            // too cheap to be worth `spawn_blocking`'s own dispatch
            // overhead, same reasoning as `read_dir` below.
            Preview {
                text: entry_preview::format_dir_preview(&child_entries(id, page)),
                override_disable_line_numbers: true,
            }
        }
    }

    async fn open(&self, id: &[String]) -> io::Result<ByteStream> {
        let absolute = self.absolute(id);
        let page = find_page(&absolute).ok_or_else(|| no_such_page(&absolute))?;
        // The whole page already lives in memory as a `&'static str` (see
        // the module docs), so there's no actual streaming to do — this
        // just satisfies `NodeSource::open`'s interface the same way `fs`'s
        // real, incremental file stream does.
        Ok(Box::pin(std::io::Cursor::new(page.body.as_bytes())))
    }

    async fn open_seekable(&self, id: &[String]) -> io::Result<SeekableByteStream> {
        let absolute = self.absolute(id);
        let page = find_page(&absolute).ok_or_else(|| no_such_page(&absolute))?;
        // `std::io::Cursor` implements `AsyncSeek` too, so this can always
        // satisfy the guarantee `open_seekable` makes.
        Ok(Box::pin(std::io::Cursor::new(page.body.as_bytes())))
    }

    async fn execute_command(&self, command: &Command, _args: &[String]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the manual has no command named {:?}", command.name),
        ))
    }
}
