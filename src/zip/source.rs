//! Stub registration of `zip://` as a [`NodeSourceType`], ahead of a real
//! zip-archive `NodeSource` existing. Its only job right now is to exercise
//! `crate::registry`'s piping plumbing (see `registry::create`'s docs) with
//! a real second participant — a node source that only makes sense as the
//! *destination* of a pipe, never the start of one, since it has no bytes
//! of its own to browse.
//!
//! When implemented, `construct_fn` will read `pipe`'s bytes (via
//! [`crate::node_source::NodeSource::open_seekable`]), hand them to
//! [`super::ZipVfs::new`], and wrap the result in
//! [`crate::fs::FsSource::with_vfs`] — same idea as `crate::fs::FsSource`
//! wrapping the real filesystem, just with the archive's *upstream* node
//! source standing in for `std::fs`. Until then every call just explains
//! why it can't do that yet, so `zip://` reads as "recognized but not
//! implemented" rather than "unrecognized scheme".

use std::io;

use crate::node_source::NodeSourceType;

/// This type's contribution to [`crate::registry::NODE_SOURCE_TYPES`].
pub static NODE_SOURCE_TYPE: NodeSourceType = NodeSourceType {
    schemes: &["zip://"],
    manual_page: None,
    commands: &[],
    toggles: &[],
    construct_fn: |_scheme, _rest, pipe| {
        if pipe.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zip:// has nothing to read on its own — pipe another node source's \
                 bytes into it, e.g. \"file://archive.zip | zip://path/inside\"",
            ));
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "zip:// is not implemented yet",
        ))
    },
    set_toggle_fn: |toggle, _value| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("zip:// has no toggle named {:?}", toggle.name),
        ))
    },
    get_toggle_fn: |toggle| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("zip:// has no toggle named {:?}", toggle.name),
        ))
    },
};
