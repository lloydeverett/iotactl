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
    /// scheme matched before handing the rest to `construct`. The
    /// filesystem type's own scheme (`"file://"`) is optional: `create`
    /// falls back to it, with the path given unaltered, when nothing else
    /// matches and the path doesn't otherwise look like a URI (see
    /// [`looks_like_uri`]).
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

/// Whether `path_arg` looks like it's *trying* to be a URI — `scheme://...`
/// — even though it matched none of [`NODE_SOURCE_TYPES`]'s schemes. Used by
/// [`create`] to tell "this is a typo'd or unsupported scheme" apart from an
/// ordinary filesystem path that just happens to contain a colon or slashes,
/// so the former can get a descriptive error instead of being handed to the
/// filesystem type and failing as a nonsense path.
///
/// Follows the scheme grammar from RFC 3986 §3.1: an alphabetic first
/// character, then any mix of letters, digits, `+`, `-`, or `.`, immediately
/// followed by `://`.
fn looks_like_uri(path_arg: &str) -> bool {
    let Some(scheme) = path_arg.split("://").next() else {
        return false;
    };
    if scheme.len() == path_arg.len() {
        return false; // no "://" present at all
    }
    let mut chars = scheme.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
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
///
/// But a `path_arg` that merely *looks* like `scheme://...` without
/// matching any registered scheme (see [`looks_like_uri`]) is never handed
/// to the filesystem type — that would just fail later as a nonsense path
/// (or worse, silently succeed against a same-named file or directory in
/// the current directory). Instead this fails fast with an error naming the
/// unrecognized scheme and what's actually available.
pub fn create(path_arg: &str) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    for &source_type in NODE_SOURCE_TYPES {
        for &scheme in source_type.schemes {
            if let Some(rest) = path_arg.strip_prefix(scheme) {
                return Ok((source_type.construct(rest)?, source_type));
            }
        }
    }
    if looks_like_uri(path_arg) {
        let scheme = path_arg.split("://").next().unwrap();
        let known: Vec<&str> = NODE_SOURCE_TYPES
            .iter()
            .flat_map(|source_type| source_type.schemes.iter().copied())
            .collect();
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unrecognized scheme \"{scheme}://\" (known schemes: {})",
                known.join(", ")
            ),
        ));
    }
    Ok((fs::NODE_SOURCE_TYPE.construct(path_arg)?, &fs::NODE_SOURCE_TYPE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_uri_accepts_a_well_formed_scheme() {
        assert!(looks_like_uri("http://example.com"));
        assert!(looks_like_uri("git+ssh://host/repo"));
        assert!(looks_like_uri("a://"));
    }

    #[test]
    fn looks_like_uri_rejects_plain_paths() {
        assert!(!looks_like_uri("some/relative/path"));
        assert!(!looks_like_uri("/etc/passwd"));
        assert!(!looks_like_uri("C:\\Users\\foo"));
        assert!(!looks_like_uri("not a scheme: still not one"));
    }

    #[test]
    fn looks_like_uri_rejects_a_scheme_starting_with_a_digit() {
        assert!(!looks_like_uri("9p://host/path"));
    }

    #[test]
    fn create_rejects_an_unrecognized_scheme_instead_of_falling_back_to_fs() {
        let Err(err) = create("bogus://something") else {
            panic!("expected create() to fail for an unrecognized scheme");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let message = err.to_string();
        assert!(message.contains("bogus://"), "message was: {message}");
        assert!(message.contains("manual://"), "message was: {message}");
    }
}
