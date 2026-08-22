//! Central place that ties a CLI path to the concrete node source it
//! selects. Each node source type (`fs`, `manual`) builds its own
//! [`NodeSourceType`] — schemes, manual page, available commands/toggles,
//! how to construct an instance, and how to get/set a toggle — and offers
//! it up via [`NODE_SOURCE_TYPES`] below; this module only aggregates and
//! dispatches across types, it never hardcodes a type's own knowledge of
//! itself.

use std::io;
use std::sync::Arc;

use crate::fs;
use crate::json;
use crate::manual;
use crate::node_source::{NodeSource, NodeSourceType};
use crate::zip;

/// Every node source type iotactl knows how to construct, each contributed
/// by its own module.
pub static NODE_SOURCE_TYPES: &[&NodeSourceType] = &[
    &manual::NODE_SOURCE_TYPE,
    &fs::NODE_SOURCE_TYPE,
    &zip::NODE_SOURCE_TYPE,
    &json::JSON_NODE_SOURCE_TYPE,
    &json::JSONL_NODE_SOURCE_TYPE,
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
/// [`create`]'s docs for the overall scheme this supports. Called on every
/// `path_arg`, whether or not it [`looks_like_uri`]: a `|` always separates
/// pipe segments now, so a literal `|` in an ordinary filename needs
/// doubling the same as one inside a URI segment does.
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
/// Fails with an error naming the unrecognized scheme and what's actually
/// available if nothing matches. No filesystem fallback here — unlike
/// [`create`], `construct_scheme` is only ever reached for a `segment` that
/// already [`looks_like_uri`], so a scheme matching nothing is a typo or an
/// unsupported scheme, never an ordinary path.
async fn construct_scheme(
    segment: &str,
    pipe: Option<Arc<dyn NodeSource>>,
) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    for &source_type in NODE_SOURCE_TYPES {
        for &scheme in source_type.schemes {
            if let Some(rest) = segment.strip_prefix(scheme) {
                return Ok((source_type.construct(scheme, rest, pipe).await?, source_type));
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
/// `path_arg` may chain sources with `|`: [`split_pipe_segments`] breaks it
/// into pieces, each built in turn and handed the previous segment's source
/// as its `pipe` (so `"file://x.zip | zip://"` builds `file://` first, then
/// `zip://` piped from it). The final segment's source is returned.
///
/// Every segment needs an explicit scheme (via [`construct_scheme`]) except
/// the leftmost one, which may omit it — `"x.zip | zip://"` is shorthand for
/// `"file://x.zip | zip://"` — since a bare path can only ever mean a real
/// filesystem path, never some other scheme's. A later segment that omits
/// its scheme is rejected instead of guessed at: `"x.zip | y.zip"` doesn't
/// mean anything (`y.zip` isn't a scheme `y` piped nothing named `.zip`, nor
/// a filesystem path — there's nothing upstream of it to read a plain path
/// against). A segment that [`looks_like_uri`] but matches no registered
/// scheme also fails fast, rather than falling through to the filesystem
/// type — that would just fail later as a nonsense path, or silently
/// succeed against an unrelated same-named file.
///
/// Splitting on `|` happens unconditionally, so a filename that genuinely
/// contains a `|` must double it (`||`) to survive — see
/// [`split_pipe_segments`].
pub async fn create(path_arg: &str) -> io::Result<(Arc<dyn NodeSource>, &'static NodeSourceType)> {
    let segments = split_pipe_segments(path_arg);
    if segments.len() == 1 && !looks_like_uri(path_arg) {
        return Ok((
            fs::NODE_SOURCE_TYPE.construct("file://", path_arg, None).await?,
            &fs::NODE_SOURCE_TYPE,
        ));
    }
    let mut pipe: Option<Arc<dyn NodeSource>> = None;
    let mut built: Option<(Arc<dyn NodeSource>, &'static NodeSourceType)> = None;
    for (i, segment) in segments.iter().enumerate() {
        let (source, source_type) = if looks_like_uri(segment) {
            construct_scheme(segment, pipe.take()).await?
        } else if i == 0 {
            (
                fs::NODE_SOURCE_TYPE.construct("file://", segment, None).await?,
                &fs::NODE_SOURCE_TYPE,
            )
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "\"{segment}\" has no scheme (only the leftmost part of a `|` \
                     pipeline may omit one; if you meant a literal \"|\" in a file \
                     name, escape it by doubling it: \"||\")"
                ),
            ));
        };
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

    #[tokio::test]
    async fn create_rejects_an_unrecognized_scheme_instead_of_falling_back_to_fs() {
        let Err(err) = create("bogus://something").await else {
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

    #[tokio::test]
    async fn create_treats_an_undoubled_pipe_as_a_segment_boundary_even_off_a_uri() {
        // Not a URI at all, but `|` is always a segment boundary now unless
        // doubled — this should be read as two segments, the second of
        // which has no scheme and so is rejected outright rather than
        // reaching the filesystem as a single literal path.
        let Err(err) = create("Cargo.toml|path").await else {
            panic!("expected create() to fail on a schemeless non-leftmost segment");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("||"), "message was: {err}");
    }

    #[tokio::test]
    async fn create_reads_a_doubled_pipe_as_one_literal_pipe_in_a_plain_path() {
        // Doubling survives even off a URI: this is a single segment whose
        // literal name contains one `|`, so it's just a nonexistent
        // filesystem path, not a two-segment pipeline.
        let Err(err) = create("some/nonexistent||path").await else {
            panic!("expected create() to fail opening a nonexistent path");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn create_permits_a_schemeless_leftmost_segment_piped_into_a_scheme() {
        // The leftmost segment may omit `file://` even when there's a pipe
        // after it — "Cargo.toml | zip://" is shorthand for
        // "file://Cargo.toml | zip://". Reaching a zip-parsing error (rather
        // than "no scheme" or "nothing piped in") proves the shorthand
        // segment was built as a filesystem source and piped forward.
        let Err(err) = create("Cargo.toml | zip://").await else {
            panic!("expected create() to fail parsing Cargo.toml as a zip archive");
        };
        assert_ne!(
            err.kind(),
            io::ErrorKind::InvalidInput,
            "expected a zip-parsing error, got: {err}"
        );
    }

    #[tokio::test]
    async fn create_rejects_a_schemeless_non_leftmost_segment() {
        // The shorthand is only for the leftmost segment: once there's
        // something upstream, every later segment needs its own scheme.
        let Err(err) = create("Cargo.toml | somefile.zip").await else {
            panic!("expected create() to fail on a schemeless non-leftmost segment");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("somefile.zip"),
            "message was: {err}"
        );
        assert!(err.to_string().contains("||"), "message was: {err}");
    }

    #[tokio::test]
    async fn create_rejects_a_pipe_into_a_type_that_does_not_support_it() {
        let Err(err) = create("manual:// | manual://").await else {
            panic!("expected create() to fail piping into manual://");
        };
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("manual://"),
            "message was: {err}"
        );
    }

    #[tokio::test]
    async fn create_rejects_zip_with_nothing_piped_into_it() {
        let Err(err) = create("zip://foo.txt").await else {
            panic!("expected create() to fail constructing zip:// with no pipe");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn create_pipes_a_constructed_source_into_the_next_segment() {
        // `manual://` always constructs successfully, but its bytes are
        // never a valid zip archive — see `crate::zip::source` for the
        // deeper, archive-mounting tests. Reaching a zip-parsing error here
        // (rather than "nothing to read on its own") is what proves the
        // `manual://` source this pipes from was actually built and its
        // bytes handed over to `zip://`, exercising `create`'s generic
        // pipe-chaining regardless of which node source types exist.
        let Err(err) = create("manual:// | zip://").await else {
            panic!("expected create() to fail parsing manual:// output as a zip archive");
        };
        assert_ne!(
            err.kind(),
            io::ErrorKind::InvalidInput,
            "expected a zip-parsing error, got: {err}"
        );
    }
}
