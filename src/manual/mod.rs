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
//! iotactl itself (e.g. navigation). A page that documents another source
//! (e.g. the filesystem source) pulls its text from that source's own
//! `docs` module (e.g. [`crate::fs::docs`]) instead of this module
//! restating it, so this module never needs to know how that source
//! actually works.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use ratatui::style::{Color, Style};

use crate::command::Command;
use crate::entry::Entry;
use crate::entry_preview;
use crate::fs;
use crate::highlight;
use crate::node_source::{Cancelled, NodeSource, Preview};
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

/// A synthetic filename handed to [`highlight::highlighted_text`] purely so
/// it infers "markdown" as the language — every manual page's `body` is
/// markdown, so this never varies per page. No file by this name is ever
/// actually opened.
const MANUAL_PAGE_PATH: &str = "manual.md";

/// One page in the manual's fixed tree.
struct ManualPage {
    /// The id segment this page is reached by, e.g. `"filesystem"`. Unused
    /// for the root page, since `id == []` reaches it directly.
    slug: &'static str,
    /// Display name, shown as the entry name in a listing.
    title: &'static str,
    /// Markdown body shown when this page is a leaf (`children` is empty).
    /// Unused for a category page, whose preview is its child listing
    /// instead — see the module docs.
    body: &'static str,
    children: &'static [ManualPage],
}

static ROOT: ManualPage = ManualPage {
    slug: "",
    title: "iotactl Manual",
    body: "",
    children: &[
        ManualPage {
            slug: "about-manual",
            title: "About This Manual",
            body: ABOUT_MANUAL,
            children: &[],
        },
        ManualPage {
            slug: "cli",
            title: "Command-Line Options",
            body: CLI_OPTIONS,
            children: &[],
        },
        ManualPage {
            slug: "navigation",
            title: "Navigation & Keybindings",
            body: NAVIGATION,
            children: &[],
        },
        ManualPage {
            slug: "node-sources",
            title: "Node Sources",
            body: NODE_SOURCES_OVERVIEW,
            children: &[],
        },
        ManualPage {
            slug: "filesystem",
            title: fs::docs::NAME,
            body: "",
            children: &[
                ManualPage {
                    slug: "overview",
                    title: "Overview",
                    body: fs::docs::OVERVIEW,
                    children: &[],
                },
                ManualPage {
                    slug: "toggles",
                    title: "Toggles",
                    body: fs::docs::TOGGLES,
                    children: &[],
                },
            ],
        },
    ],
};

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
- Give `manual://` to open this manual instead.

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
/// segment against a child's `slug`. Returns `None` if any segment doesn't
/// match — the same "no such node" case a real source would report if a
/// caller handed it a stale or invalid id.
fn find_page(id: &[String]) -> Option<&'static ManualPage> {
    let mut page = &ROOT;
    for segment in id {
        page = page.children.iter().find(|child| child.slug == segment)?;
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

fn child_entries(id: &[String], page: &ManualPage) -> Vec<Entry> {
    page.children
        .iter()
        .map(|child| {
            let mut child_id = id.to_vec();
            child_id.push(child.slug.to_string());
            let is_dir = !child.children.is_empty();
            let (icon, icon_color) = entry_icon(is_dir);
            Entry {
                name: child.title.to_string(),
                id: child_id,
                is_dir,
                is_link: false,
                suggested_commands: Arc::from(Vec::new()),
                nerd_icon: Some(icon),
                nerd_icon_color: icon_color,
            }
        })
        .collect()
}

/// A `NodeSource` that shows the fixed manual tree defined in this module.
pub struct ManualSource {
    /// Threaded down from the `--nerd-font` CLI flag, purely so this
    /// source's own directory-style previews (see `preview_tui`) pad their
    /// entries the same way every other source's listing does.
    nerd_font: bool,
    /// Whether a leaf page's preview shows its markdown marks as-is (`true`)
    /// or rendered/hidden (`false`, the default) — see
    /// `crate::highlight::RAW_TOGGLE_NAME`. Not shared via `Arc` like `fs`'s
    /// toggle state: `ManualSource` is never cloned, since its
    /// `spawn_blocking`'d work (see `preview_tui`) only ever needs the two
    /// `Copy` values that work actually depends on (the toggle's current
    /// value, read here with `&self` before dispatch, and the page's
    /// `&'static str` body), not a clone of the whole source the way `fs`
    /// needs for its blocking closures to have owned access to `self`.
    raw_markdown: AtomicBool,
}

impl ManualSource {
    pub fn new(nerd_font: bool) -> Self {
        ManualSource {
            nerd_font,
            raw_markdown: AtomicBool::new(false),
        }
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
        let page = find_page(id).ok_or_else(|| no_such_page(id))?;
        Ok(child_entries(id, page))
    }

    async fn root_entry(&self) -> Entry {
        let (icon, icon_color) = entry_icon(true);
        Entry {
            name: ROOT.title.to_string(),
            id: Vec::new(),
            is_dir: true,
            is_link: false,
            suggested_commands: Arc::from(Vec::new()),
            nerd_icon: Some(icon),
            nerd_icon_color: icon_color,
        }
    }

    async fn preview_tui(&self, id: &[String], _cancelled: &Cancelled) -> Preview {
        let Some(page) = find_page(id) else {
            return Preview::new(error_text(no_such_page(id).to_string()));
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
            let hide_markers = !self.raw_markdown.load(Ordering::SeqCst);
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
                text: entry_preview::format_dir_preview(&child_entries(id, page), self.nerd_font),
                override_disable_line_numbers: true,
            }
        }
    }

    fn available_commands(&self) -> Arc<[Command]> {
        Arc::from(Vec::new())
    }

    fn available_toggles(&self) -> Arc<[Toggle]> {
        Arc::from(vec![Toggle {
            name: highlight::RAW_TOGGLE_NAME.to_string(),
            key: highlight::RAW_TOGGLE_KEY,
        }])
    }

    async fn set_toggle(&self, toggle: &Toggle, value: bool) -> io::Result<()> {
        if toggle.name == highlight::RAW_TOGGLE_NAME {
            self.raw_markdown.store(value, Ordering::SeqCst);
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the manual has no toggle named {:?}", toggle.name),
            ))
        }
    }

    async fn get_toggle(&self, toggle: &Toggle) -> io::Result<bool> {
        if toggle.name == highlight::RAW_TOGGLE_NAME {
            Ok(self.raw_markdown.load(Ordering::SeqCst))
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the manual has no toggle named {:?}", toggle.name),
            ))
        }
    }

    async fn execute_command(&self, command: &Command, _args: &[String]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the manual has no command named {:?}", command.name),
        ))
    }
}
