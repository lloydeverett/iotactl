//! A [`crate::fs::vfs::Vfs`] implementation over the contents of a zip
//! archive, so `fs`'s browsing/preview logic (directory-first sorting,
//! syntax highlighting, the meta/raw toggles, ...) works the same way over
//! an archive's contents as it does over a real directory tree, without
//! extracting it to disk first. See [`ZipVfs`]'s docs for the design, and
//! `AGENTS.md` for how this fits the project's eventual goal of
//! non-filesystem-based node sources.
//!
//! This module only provides the `Vfs` implementation, not a full
//! `NodeSource` — nothing here decides what scheme addresses a zip archive
//! on the CLI, or how its root is chosen. A future node source would wrap
//! [`ZipVfs`] behind `crate::fs::FsSource::with_vfs`, handing it a
//! [`crate::node_source::SeekableByteStream`] opened from wherever the
//! archive's bytes actually live.

mod archive;
mod bridge;
mod entry_stream;
mod index;
#[cfg(test)]
mod tests;

pub use archive::ZipVfs;
