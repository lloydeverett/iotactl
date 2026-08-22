//! Documentation content for `zip`, published as a [`ManualPage`] so
//! `crate::registry` can hand it to the `manual` node source. Written in
//! markdown, and in ASD-STE100 style (short sentences, one idea per
//! sentence, plain words) since it is user-facing — see `crate::fs::docs`
//! for the same pattern.

use crate::node_source::ManualPage;

/// Title for this source's section of the manual.
const NAME: &str = "Zip Archives";

/// What this source is and how it is scoped.
const OVERVIEW: &str = "\
# Zip Archives

The zip source browses the contents of a zip archive, without extracting it to
disk first.

## Opening an archive

`zip://` has nothing to read on its own. Pipe another source's bytes into it
with `|`, for example:

`iotactl \"file://archive.zip | zip://\"`

Add a path after `zip://` to start browsing inside the archive instead of at
its root, for example `zip://some/folder`.

The source piped into `zip://` must support seeking. A real file does. A
stream that only reads forward once, such as another process's output, does
not.

## Browsing and toggles

Once mounted, an archive browses the same way the filesystem source does:
directories first, then files, sorted without regard to upper or lower case.
It has the same toggles, too. See the \"Filesystem\" topic's \"Toggles\" page.";

/// This source's contribution to the manual: itself as a top-level topic,
/// with `OVERVIEW` as its only page.
pub static MANUAL_PAGE: ManualPage = ManualPage {
    slug: "zip",
    title: NAME,
    body: "",
    children: &[&ManualPage {
        slug: "overview",
        title: "Overview",
        body: OVERVIEW,
        children: &[],
    }],
};
