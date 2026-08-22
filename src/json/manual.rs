//! Documentation content for `json` and `jsonl`, published as
//! [`ManualPage`]s so `crate::registry` can hand them to the `manual` node
//! source. Written in markdown, and in ASD-STE100 style (short sentences,
//! one idea per sentence, plain words) since it is user-facing — see
//! `crate::fs::docs` for the same pattern.

use crate::node_source::ManualPage;

/// Title for the JSON source's section of the manual.
const JSON_NAME: &str = "JSON";

/// What the JSON source is and how it is scoped.
const JSON_OVERVIEW: &str = "\
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
positions. When you select a leaf value, the preview shows that value.

## JSON Lines

See the \"JSON Lines\" topic in this manual for `jsonl://`, a sibling source
for documents with one JSON value per line.";

/// This source's contribution to the manual: itself as a top-level topic,
/// with `JSON_OVERVIEW` as its only page.
pub static JSON_MANUAL_PAGE: ManualPage = ManualPage {
    slug: "json",
    title: JSON_NAME,
    body: "",
    children: &[&ManualPage {
        slug: "overview",
        title: "Overview",
        body: JSON_OVERVIEW,
        children: &[],
    }],
};

/// Title for the JSON Lines source's section of the manual.
const JSONL_NAME: &str = "JSON Lines";

/// What the JSON Lines source is and how it is scoped.
const JSONL_OVERVIEW: &str = "\
# JSON Lines

The JSON Lines source (jsonl) reads a document in the JSON Lines format: one
JSON value per line. It puts these values into a list, in line order, then
browses that list the same way the JSON source browses a JSON array. See the
\"JSON\" topic in this manual for how a list, an object, and a leaf value
each preview.

## Opening a document

`jsonl://` has nothing to read on its own. Pipe another source's bytes into
it with `|`, for example:

`iotactl \"file://data.jsonl | jsonl://\"`

Each line becomes one item in the list, numbered from 0. Add a path after
`jsonl://` to start browsing inside one item instead of at the list's root,
for example `jsonl://0` for the first line, or `jsonl://0/some/key` for a key
inside it.

A blank line does not become an item. iotactl skips it.

## Malformed lines

If a line does not parse as a single JSON value, opening the document fails.
The error names the line number.";

/// This source's contribution to the manual: itself as a top-level topic,
/// with `JSONL_OVERVIEW` as its only page.
pub static JSONL_MANUAL_PAGE: ManualPage = ManualPage {
    slug: "jsonl",
    title: JSONL_NAME,
    body: "",
    children: &[&ManualPage {
        slug: "overview",
        title: "Overview",
        body: JSONL_OVERVIEW,
        children: &[],
    }],
};
