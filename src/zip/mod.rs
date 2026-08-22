//! A [`crate::fs::vfs::Vfs`] implementation over the contents of a zip
//! archive, so `fs`'s browsing/preview logic (directory-first sorting,
//! syntax highlighting, the meta/raw toggles, ...) works the same way over
//! an archive's contents as it does over a real directory tree, without
//! extracting it to disk first. See [`ZipVfs`]'s docs for the design, and
//! `AGENTS.md` for how this fits the project's eventual goal of
//! non-filesystem-based node sources.
//!
//! Alongside `Vfs`, this module contributes a `zip://`
//! [`NodeSourceType`](crate::node_source::NodeSourceType) (see [`source`]) —
//! for now just a stub, since the real thing needs
//! [`ZipVfs`] wrapped in `crate::fs::FsSource::with_vfs` over a
//! [`crate::node_source::SeekableByteStream`] piped in from another node
//! source (see `crate::registry::create`'s pipe-parsing), not just an
//! `Arc<dyn Vfs>` constructed from a CLI path the way every other type here
//! builds itself.

mod archive;
mod bridge;
mod entry_stream;
mod index;
mod source;
#[cfg(test)]
mod tests;

pub use archive::ZipVfs;
pub use source::NODE_SOURCE_TYPE;
