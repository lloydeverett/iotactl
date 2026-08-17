//! Central place that ties a CLI path to the concrete node source it
//! selects. Each node source type (`fs`, `manual`) builds its own
//! [`NodeSourceType`] — schemes, manual page, available commands/toggles,
//! how to construct an instance, and how to get/set a toggle — and offers
//! it up via [`NODE_SOURCE_TYPES`] below; this module only aggregates and
//! dispatches across types, it never hardcodes a type's own knowledge of
//! itself.

use std::io;
use std::sync::Arc;

use crate::command::Command;
use crate::fs;
use crate::manual;
use crate::node_source::{ManualPage, NodeSource};
use crate::toggle::Toggle;

/// Describes one kind of node source iotactl can browse, independent of
/// any particular instance of it.
///
/// Toggle get/set live here rather than on [`NodeSource`] because toggle
/// state, for every source type iotactl currently has, is process-global
/// (see e.g. `fs`'s module-level `SHOW_HIDDEN`) — there's only ever one
/// source in use at a time, so there's nothing per-instance to hold. That
/// also means a toggle name can be validated (e.g. a `--toggle-on NAME` at
/// startup — see `crate::app::App::new`) against every registered type at
/// once, via [`toggle_known`], without constructing any of them.
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
    /// The commands this type of source makes available, independent of
    /// any particular node. Not yet invoked anywhere.
    pub commands: &'static [Command],
    /// The toggles this type of source makes available, independent of
    /// any particular node.
    pub toggles: &'static [Toggle],
    /// Builds this type's source given the CLI path with its matched scheme
    /// (if any) already stripped. Any finer addressing within the source
    /// than just picking which type to use (e.g. `manual://filesystem`
    /// picking a page, not just the manual type) is the source's own job to
    /// bake into its scope at construction — see `ManualSource::new`'s
    /// `root` parameter — not something `construct` reports back out.
    pub(crate) construct_fn: fn(&str) -> io::Result<Arc<dyn NodeSource>>,
    /// Sets `toggle` (one of this type's own `toggles`) to `value`.
    pub(crate) set_toggle_fn: fn(&Toggle, bool) -> io::Result<()>,
    /// Reads the current value of `toggle` (one of this type's own
    /// `toggles`).
    pub(crate) get_toggle_fn: fn(&Toggle) -> io::Result<bool>,
}

impl NodeSourceType {
    /// See `construct_fn`'s docs.
    pub fn construct(&self, rest: &str) -> io::Result<Arc<dyn NodeSource>> {
        (self.construct_fn)(rest)
    }

    /// Whether this type exposes a toggle named `name` — as opposed to
    /// [`toggle_known`], which checks every registered type at once.
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

/// Every node source type iotactl knows how to construct, each contributed
/// by its own module.
pub static NODE_SOURCE_TYPES: &[&NodeSourceType] = &[&manual::NODE_SOURCE_TYPE, &fs::NODE_SOURCE_TYPE];

/// Whether *some* known node source type — not necessarily the one
/// currently in use — exposes a toggle named `name`. Lets a caller (e.g.
/// `App::new`, validating `--toggle-on`/`--toggle-off`) distinguish "this
/// toggle isn't recognized by anything" from "this toggle exists, just not
/// on the source you're currently browsing" — only the former is worth
/// failing startup over.
pub fn toggle_known(name: &str) -> bool {
    NODE_SOURCE_TYPES.iter().any(|source_type| source_type.has_toggle(name))
}

/// Splits the part of a CLI path after its scheme into id segments — the
/// same shape [`crate::node_source::NodeSource::read_dir`] takes. Ignores
/// empty segments (a leading, trailing, or doubled `/`) rather than
/// erroring, so e.g. `manual://filesystem/` behaves the same as
/// `manual://filesystem`.
pub(crate) fn split_id(rest: &str) -> Vec<String> {
    rest.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Constructs the node source for `path_arg`, alongside the
/// [`NodeSourceType`] that turned out to describe it.
///
/// Tries each type's `schemes` as a prefix of `path_arg` in turn; on a
/// match, that type's `construct` builds the source from the rest of the
/// path. A `path_arg` matching no scheme at all falls back to the
/// filesystem type with the whole path — unlike every other type, its
/// scheme is optional, so this fallback is special-cased here rather than
/// driven by `NODE_SOURCE_TYPES` the way scheme matches are.
pub fn create(path_arg: &str) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    for &source_type in NODE_SOURCE_TYPES {
        for &scheme in source_type.schemes {
            if let Some(rest) = path_arg.strip_prefix(scheme) {
                return Ok((source_type.construct(rest)?, source_type));
            }
        }
    }
    Ok((fs::NODE_SOURCE_TYPE.construct(path_arg)?, &fs::NODE_SOURCE_TYPE))
}
