//! Two `NodeSource` types that browse a parsed JSON document as a tree: an
//! object's keys and an array's indices are child nodes, and a scalar
//! (string, number, bool, or null) is a leaf whose preview is its own value.
//!
//! `json://` parses its piped-in bytes as a single JSON document. `jsonl://`
//! instead parses them as [JSON Lines](https://jsonlines.org/) — one JSON
//! value per line — and presents the lines as a JSON array, one element per
//! line; from there on it browses exactly like `json://` does (see
//! `source::JsonSource`). Both only make sense piped from another node
//! source (see `crate::registry::create`'s pipe-parsing) — there's nowhere
//! for their bytes to come from otherwise: `"file://data.json | json://"`
//! opens the document's root, `"file://data.json | json://some/key"` opens a
//! node within it. The whole document is parsed once, up front (see
//! `source::JSON_NODE_SOURCE_TYPE`/`JSONL_NODE_SOURCE_TYPE`'s
//! `construct_fn`s), so every other operation just walks the resulting tree
//! in memory rather than re-reading or re-parsing anything.
//!
//! Unlike `crate::zip`, this doesn't reuse `crate::fs`'s directory/file
//! machinery: a JSON key can contain characters (`/`, `.`, `..`, an empty
//! string) that `fs::FsSource::path_from_segments` would reject as a path
//! segment, and a JSON node has no analogue for a symlink or Unix
//! permissions. So this module implements `NodeSource` directly instead,
//! indexing into the parsed tree by raw id segments the same way
//! `crate::manual` walks its fixed page tree.

mod manual;
mod source;

pub use source::{JSONL_NODE_SOURCE_TYPE, JSON_NODE_SOURCE_TYPE};
