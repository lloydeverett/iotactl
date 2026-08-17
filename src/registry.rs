//! Central place that knows how to turn a CLI path into a concrete
//! [`NodeSource`], and the only module (besides [`crate::fs`] and
//! [`crate::manual`] themselves) allowed to name their concrete types
//! ([`FsSource`], [`ManualSource`]) or their schemes. Every other caller —
//! `main`, `manual` — goes through [`create`] or [`NODE_SOURCE_TYPES`]
//! instead, so that adding a new node source type only ever touches this
//! file (plus, if it wants a manual page, its own `docs` module — see
//! [`crate::fs::docs`] for the pattern).

use std::io;
use std::sync::Arc;

use crate::fs::{self, FsSource};
use crate::manual::ManualSource;
use crate::node_source::{ManualPage, NodeSource};

/// Describes one kind of node source iotactl can browse.
pub struct NodeSourceType {
    /// URI schemes that select this type on the CLI path (e.g.
    /// `"manual://"`), matched by prefix — [`create`] strips whichever
    /// scheme matched before handing the rest to `construct`. Empty for the
    /// filesystem type: it has no scheme of its own, since `create` falls
    /// back to it, with the path given unaltered, when nothing else matches.
    pub schemes: &'static [&'static str],
    /// Manual content this type contributes about itself, embedded as a
    /// top-level topic in the manual's page tree (see [`crate::manual`]).
    /// `None` for a type that doesn't describe itself there — the manual
    /// type itself, whose own pages already are the manual.
    pub manual_page: Option<&'static ManualPage>,
    /// Builds this type's source given the CLI path with its matched scheme
    /// (if any) already stripped. Any finer addressing within the source
    /// than just picking which type to use (e.g. `manual://filesystem`
    /// picking a page, not just the manual type) is the source's own job to
    /// bake into its scope at construction — see `ManualSource::new`'s
    /// `root` parameter — not something `construct` reports back out.
    construct: fn(&str) -> io::Result<Arc<dyn NodeSource>>,
}

/// Every node source type iotactl knows how to construct.
pub static NODE_SOURCE_TYPES: &[NodeSourceType] = &[
    NodeSourceType {
        // A prefix, not just an exact match: anything after it addresses a
        // page within the manual (e.g. `manual://filesystem`), split on
        // `/` into an id — the same shape `find_page` walks for ordinary
        // in-app navigation. Empty for bare `manual://`, meaning "start at
        // the top".
        schemes: &["manual://"],
        manual_page: None,
        construct: |rest| Ok(Arc::new(ManualSource::new(split_id(rest))?)),
    },
    NodeSourceType {
        // Optional: a path with no recognized scheme at all also selects
        // this type (see `create`'s fallback), so this only matters for a
        // path that would otherwise be ambiguous. Consumed whole as the
        // real filesystem path to browse, unlike the manual scheme —
        // never split into an id, since the given path already picks out
        // exactly one node (no separate "start" needed).
        schemes: &["file://"],
        manual_page: Some(&fs::docs::MANUAL_PAGE),
        construct: |rest| Ok(Arc::new(FsSource::new(rest)?)),
    },
];

/// Splits the part of a CLI path after its scheme into id segments — the
/// same shape [`crate::node_source::NodeSource::read_dir`] takes. Ignores
/// empty segments (a leading, trailing, or doubled `/`) rather than
/// erroring, so e.g. `manual://filesystem/` behaves the same as
/// `manual://filesystem`.
fn split_id(rest: &str) -> Vec<String> {
    rest.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Constructs the node source for `path_arg`, hiding which concrete type
/// that turned out to be from the caller.
///
/// Tries each type's `schemes` as a prefix of `path_arg` in turn; on a
/// match, that type's `construct` builds the source from the rest of the
/// path. A `path_arg` matching no scheme at all falls back to the
/// filesystem type with the whole path — unlike every other type, its
/// scheme is optional, so this fallback is special-cased here rather than
/// driven by `NODE_SOURCE_TYPES` the way scheme matches are.
pub fn create(path_arg: &str) -> io::Result<Arc<dyn NodeSource>> {
    for source_type in NODE_SOURCE_TYPES {
        for &scheme in source_type.schemes {
            if let Some(rest) = path_arg.strip_prefix(scheme) {
                return (source_type.construct)(rest);
            }
        }
    }
    Ok(Arc::new(FsSource::new(path_arg)?))
}
