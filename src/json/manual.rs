//! Documentation content for `json`, published as a [`ManualPage`] so
//! `crate::registry` can hand it to the `manual` node source. Written in
//! markdown, and in ASD-STE100 style (short sentences, one idea per
//! sentence, plain words) since it is user-facing — see `crate::fs::docs`
//! for the same pattern.

use crate::node_source::ManualPage;

/// Title for this source's section of the manual.
const NAME: &str = "JSON";

/// What this source is and how it is scoped.
const OVERVIEW: &str = "\
# JSON

The JSON source browses a JSON document as a tree. An object's keys, and an
array's positions, are child nodes. A string, number, boolean, or null value
is a leaf. Select a leaf to see its value.

## Opening a document

`json://` has nothing to read on its own. Pipe another source's bytes into it
with `|`, for example:

`iotactl \"file://data.json | json://\"`

Add a path after `json://` to start browsing inside the document instead of
at its root, for example `json://some/key`. Use a plain position number for an
array index, for example `json://some/list/0`.

A key that itself contains a `/` cannot be reached this way. Open the
document at its root instead, then navigate to that key inside iotactl.

## Order and previews

Object keys keep the order they had in the document. Array positions keep
their index order. Neither is re-sorted.

When you select an object or an array, the preview shows its child keys or
positions. When you select a leaf value, the preview shows that value.";

/// This source's contribution to the manual: itself as a top-level topic,
/// with `OVERVIEW` as its only page.
pub static MANUAL_PAGE: ManualPage = ManualPage {
    slug: "json",
    title: NAME,
    body: "",
    children: &[&ManualPage {
        slug: "overview",
        title: "Overview",
        body: OVERVIEW,
        children: &[],
    }],
};
