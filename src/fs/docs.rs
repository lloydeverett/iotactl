//! Documentation content for `fs`, published as a [`ManualPage`] so
//! [`crate::registry`] can hand it to the `manual` node source. `fs` has no
//! knowledge that `manual` exists — it only publishes plain, inert data
//! here. That data is shown as markdown (see
//! `crate::highlight::highlighted_text`), so it is written in markdown, and
//! in ASD-STE100 style (short sentences, one idea per sentence, plain
//! words) since it is user-facing.
//!
//! Keep this in sync by hand with the real toggle names/keys in `super`
//! (`HIDDEN_TOGGLE_NAME` and friends, and `crate::highlight::RAW_TOGGLE_NAME`)
//! if those ever change.

use crate::node_source::ManualPage;

/// Title for this source's section of the manual.
const NAME: &str = "Filesystem";

/// What this source is and how it is scoped.
const OVERVIEW: &str = "\
# Filesystem

The filesystem source shows real files and directories on disk. It is the
default source. iotactl uses it when you give a real path, or no path at all.

## Root directory

The source starts at one root directory.

- If you give a path on the command line, iotactl uses that path as the root.
- If you give no path, iotactl uses the current directory as the root.

You cannot move above the root directory. This keeps browsing inside the part of
the disk you asked for.

## Listings

A directory listing shows directories first, then files. Within each group,
iotactl sorts names without regard to upper or lower case.

## Previews

When you select a directory, the preview shows its listing.

When you select a file, the preview shows the file text. iotactl adds color for
file types it knows, such as Rust, Python, JSON, and Markdown.

A file with raw binary bytes has no text preview. iotactl shows its size
instead.";

/// The toggles this source exposes (see `available_toggles`).
const TOGGLES: &str = "\
# Filesystem Toggles

A toggle turns a feature on or off. Each toggle has one key. Press `t` to open
the toggles menu. The toggles menu lists every toggle and its key.

## Key `H`: hidden

This toggle shows or hides dotfiles. A dotfile is a file with a name that starts
with a dot, such as `.gitignore`. The hidden toggle is off by default, so
dotfiles stay hidden until you turn it on.

## Key `r`: raw

This toggle shows or hides markdown marks in a preview. A markdown mark is a
character such as `#` or `*`. It shows the file's raw text, marks and all, when
on. It hides the marks, and shows only the styled text, when off. The raw toggle
is off by default.

This is the same toggle, with the same key, that this manual uses for its own
pages. See the manual's own \"About This Manual\" page.

## Key `m`: meta

This toggle shows a node's metadata instead of its normal preview content.
Metadata includes:

- Size
- Permissions
- Owner
- Timestamps

The meta toggle is off by default.";

/// This source's contribution to the manual: itself as a top-level topic,
/// with `OVERVIEW` and `TOGGLES` as its two pages. Handed to the manual node
/// source only indirectly, via `crate::registry::NodeSourceType`.
pub static MANUAL_PAGE: ManualPage = ManualPage {
    slug: "filesystem",
    title: NAME,
    body: "",
    children: &[
        &ManualPage {
            slug: "overview",
            title: "Overview",
            body: OVERVIEW,
            children: &[],
        },
        &ManualPage {
            slug: "toggles",
            title: "Toggles",
            body: TOGGLES,
            children: &[],
        },
    ],
};
