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
use crate::zip;

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
    /// `"manual://"`), matched by prefix — [`create`] hands both the
    /// matched scheme and the rest of the path to `construct`. The
    /// filesystem type's own scheme (`"file://"`) is optional: `create`
    /// falls back to it, with the path given unaltered as `rest`, when
    /// nothing else matches and the path doesn't otherwise look like a URI
    /// (see [`looks_like_uri`]).
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
    /// Builds this type's source given the scheme that selected it (e.g.
    /// `"file://"`, always non-empty and one of this type's own `schemes`
    /// even when the CLI path itself had none — see [`create`]'s fallback)
    /// and the rest of the CLI path after that scheme. Handing both pieces
    /// over, rather than a single pre-stripped string, keeps `create` from
    /// having to special-case how much of the path a given type gets to
    /// see; it's this function's own job to decide what to do with them.
    /// Any finer addressing within the source than just picking which type
    /// to use (e.g. `manual://filesystem` picking a page, not just the
    /// manual type) is the source's own job to bake into its scope at
    /// construction — see `ManualSource::new`'s `root` parameter — not
    /// something `construct` reports back out.
    ///
    /// `pipe` is the node source that piped into this one — `Some` when
    /// this segment wasn't the first in a `|`-delimited path (see
    /// [`create`]'s pipe-parsing), `None` otherwise. Only some types make
    /// sense as a pipe's destination (e.g. a future `zip://`, whose bytes
    /// have to come from somewhere); a type that doesn't should reject a
    /// `Some` here rather than silently ignoring it, and a type that
    /// *requires* piping (nothing to browse on its own) should reject
    /// `None` the same way. `fs` and `manual` both reject `Some`.
    pub(crate) construct_fn:
        fn(&str, &str, Option<Arc<dyn NodeSource>>) -> io::Result<Arc<dyn NodeSource>>,
    /// Sets `toggle` (one of this type's own `toggles`) to `value`.
    pub(crate) set_toggle_fn: fn(&Toggle, bool) -> io::Result<()>,
    /// Reads the current value of `toggle` (one of this type's own
    /// `toggles`).
    pub(crate) get_toggle_fn: fn(&Toggle) -> io::Result<bool>,
}

impl NodeSourceType {
    /// See `construct_fn`'s docs.
    pub fn construct(
        &self,
        scheme: &str,
        rest: &str,
        pipe: Option<Arc<dyn NodeSource>>,
    ) -> io::Result<Arc<dyn NodeSource>> {
        (self.construct_fn)(scheme, rest, pipe)
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
pub static NODE_SOURCE_TYPES: &[&NodeSourceType] = &[
    &manual::NODE_SOURCE_TYPE,
    &fs::NODE_SOURCE_TYPE,
    &zip::NODE_SOURCE_TYPE,
];

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

/// Splits `path_arg` on unescaped `|` characters into pipe segments — see
/// [`create`]'s docs for the overall scheme this supports. Only ever called
/// on a `path_arg` that [`looks_like_uri`]; the fallback filesystem case
/// never splits on `|` at all, so a real filename containing one is never
/// mangled there.
///
/// A `|` is a segment boundary unless doubled (`||`), which decodes to one
/// literal `|` within a segment instead — the only way to address a node
/// whose own path genuinely contains a pipe character. Doubling is resolved
/// greedily, left to right, so `"a|||b"` splits into `"a|"` and `"b"`, not
/// `"a"` and `"|b"`. Each segment is trimmed of surrounding ASCII
/// whitespace, so `"file://x.zip | zip://y"` reads the same as
/// `"file://x.zip|zip://y"` — the spaces are purely for readability.
fn split_pipe_segments(path_arg: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = path_arg.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '|' {
            if chars.peek() == Some(&'|') {
                chars.next();
                current.push('|');
            } else {
                segments.push(current.trim().to_string());
                current = String::new();
            }
        } else {
            current.push(c);
        }
    }
    segments.push(current.trim().to_string());
    segments
}

/// Matches `segment` — one pipe segment from [`split_pipe_segments`], or a
/// whole non-piped `path_arg` — against every registered type's `schemes`
/// as a prefix, constructing the matching type with `pipe` as its upstream
/// source (see `NodeSourceType::construct_fn`'s docs for what that means).
///
/// Fails the way [`create`] used to when nothing matches: with an error
/// naming the unrecognized scheme and what's actually available. Unlike
/// `create`'s old fallback, there's no filesystem special case here —
/// `construct_scheme` is only ever reached for something that already
/// [`looks_like_uri`], so a `segment` matching no scheme is a typo or an
/// unsupported scheme, never an ordinary path.
fn construct_scheme(
    segment: &str,
    pipe: Option<Arc<dyn NodeSource>>,
) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    for &source_type in NODE_SOURCE_TYPES {
        for &scheme in source_type.schemes {
            if let Some(rest) = segment.strip_prefix(scheme) {
                return Ok((source_type.construct(scheme, rest, pipe)?, source_type));
            }
        }
    }
    let scheme = segment.split("://").next().unwrap_or(segment);
    let known: Vec<&str> = NODE_SOURCE_TYPES
        .iter()
        .flat_map(|source_type| source_type.schemes.iter().copied())
        .collect();
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "unrecognized scheme \"{scheme}://\" (known schemes: {})",
            known.join(", ")
        ),
    ))
}

/// Constructs the node source for `path_arg`, alongside the
/// [`NodeSourceType`] that turned out to describe it.
///
/// A `path_arg` that [`looks_like_uri`] may additionally chain node sources
/// together with `|`: [`split_pipe_segments`] breaks it into pieces, each
/// constructed in turn via [`construct_scheme`] and handed the previous
/// segment's freshly built source as its `pipe` — so
/// `"file://x.zip | zip://foo.txt"` builds the `file://` source first, then
/// builds the `zip://` source with that `file://` source as its `pipe`
/// (unimplemented today, see `crate::zip`, but already reachable through
/// this path). The final segment's source is what's returned. A single,
/// unpiped `path_arg` is just the one-segment case of the same loop.
///
/// A `path_arg` that doesn't look like a URI at all skips every bit of the
/// above — no splitting, no `||` decoding, `|` is just an ordinary
/// character — and falls back to the filesystem type unaltered, unlike
/// every other type, its scheme is optional. But `construct` still gets
/// called with a scheme, `"file://"`, and `rest` set to the whole path, so
/// the fallback looks like an ordinary match rather than a special case
/// only this function knows about.
///
/// A `path_arg` that merely *looks* like `scheme://...` (or, for a later
/// pipe segment, a segment that looks like one) without matching any
/// registered scheme is never handed to the filesystem type — that would
/// just fail later as a nonsense path (or worse, silently succeed against a
/// same-named file or directory in the current directory). Instead this
/// fails fast with an error naming the unrecognized scheme and what's
/// actually available.
pub fn create(path_arg: &str) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    if !looks_like_uri(path_arg) {
        return Ok((
            fs::NODE_SOURCE_TYPE.construct("file://", path_arg, None)?,
            &fs::NODE_SOURCE_TYPE,
        ));
    }
    let mut pipe: Option<Arc<dyn NodeSource>> = None;
    let mut built: Option<(Arc<dyn NodeSource>, &'static NodeSourceType)> = None;
    for segment in split_pipe_segments(path_arg) {
        let (source, source_type) = construct_scheme(&segment, pipe.take())?;
        pipe = Some(Arc::clone(&source));
        built = Some((source, source_type));
    }
    Ok(built.expect("split_pipe_segments always yields at least one segment"))
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

    #[test]
    fn split_pipe_segments_splits_and_trims() {
        assert_eq!(
            split_pipe_segments("file://x.zip | zip://foo.txt"),
            vec!["file://x.zip", "zip://foo.txt"]
        );
        assert_eq!(split_pipe_segments("a://b"), vec!["a://b"]);
    }

    #[test]
    fn split_pipe_segments_decodes_doubled_pipes_greedily() {
        assert_eq!(split_pipe_segments("a||b"), vec!["a|b"]);
        assert_eq!(split_pipe_segments("a|||b"), vec!["a|", "b"]);
    }

    #[test]
    fn create_does_not_split_on_pipe_for_a_plain_filesystem_path() {
        // Not a URI at all, so `|` is just an ordinary filename character —
        // this should try (and fail) to open a single literal path rather
        // than being split into pipe segments.
        let Err(err) = create("some/nonexistent|path") else {
            panic!("expected create() to fail opening a nonexistent path");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn create_rejects_a_pipe_into_a_type_that_does_not_support_it() {
        let Err(err) = create("manual:// | manual://") else {
            panic!("expected create() to fail piping into manual://");
        };
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("manual://"),
            "message was: {err}"
        );
    }

    #[test]
    fn create_rejects_zip_with_nothing_piped_into_it() {
        let Err(err) = create("zip://foo.txt") else {
            panic!("expected create() to fail constructing zip:// with no pipe");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn create_pipes_a_constructed_source_into_the_next_segment() {
        // zip:// is only a stub today, but reaching its "not implemented"
        // error (rather than "nothing to read on its own") proves the
        // manual:// source it's piped from was built and handed over.
        let Err(err) = create("manual:// | zip://foo.txt") else {
            panic!("expected create() to fail on the unimplemented zip:// stub");
        };
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("not implemented"),
            "message was: {err}"
        );
    }
}
