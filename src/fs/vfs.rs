//! The interface [`FsSource`](super::FsSource) uses for every operation
//! that would otherwise be a direct `std::fs`/`tokio::fs` call: listing a
//! directory, reading metadata, following a symlink, reading a file's
//! bytes. `FsSource` itself holds only an `Arc<dyn Vfs>` and never
//! touches `std::fs` directly (see `super::real` for the one implementation
//! that does) — the rest of `fs`'s code (hidden-file filtering, icon
//! lookup, sorting, the meta/raw preview toggles) is policy that applies
//! the same way no matter where the bytes actually come from, so none of
//! it needs to change if a future node source wants the same browsing/
//! preview behavior over something that isn't the real filesystem at all
//! (e.g. the contents of a zip file, addressed the same way a directory
//! tree would be).
//!
//! Every method here mirrors a real filesystem primitive closely enough
//! that a real-fs-backed implementation is a thin wrapper (see
//! `super::real::RealVfs`), but returns this module's own
//! [`Metadata`]/[`DirEntryInfo`] rather than `std::fs`'s types, since those
//! are tied to an actual `Metadata`/`DirEntry` on a real inode and couldn't
//! be constructed by a source with no real filesystem underneath it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::node_source::Cancelled;
use crate::streams::{ByteStream, SeekableByteStream};

/// Unix-only metadata fields, held separately from [`Metadata`] rather than
/// inlined into it since they have no meaningful value on a non-Unix
/// platform (or a backend with no concept of a Unix uid/gid/mode at all,
/// e.g. one built over a zip file) — `None` there rather than a fabricated
/// zero.
#[derive(Clone, Copy, Debug)]
pub struct UnixMetadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// A backend-agnostic stand-in for `std::fs::Metadata`, covering every
/// field `fs` actually reads from one (see `preview_meta` and
/// `preview_tui_sync` in `super`). Returned by both [`Vfs::metadata`]
/// (follows a symlink) and [`Vfs::symlink_metadata`] (does not),
/// mirroring `stat`/`lstat`.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    /// `None` on a non-Unix platform, or for a backend with nothing
    /// sensible to report here. See [`UnixMetadata`].
    pub unix: Option<UnixMetadata>,
}

/// One entry from [`Vfs::read_dir`]: just enough to build an
/// [`crate::entry::Entry`] (name, whether it's a directory, whether it's a
/// symlink) without handing back a full [`Metadata`] per entry, since
/// `read_dir` is called for every listing (including the directory-preview
/// path, which can run over a large directory) and a source may have a
/// cheaper way to answer "is this a dir" while scanning than a full stat
/// per child.
#[derive(Clone, Debug)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
}

/// Abstracts every filesystem-shaped operation `fs` needs, so `FsSource`
/// can be pointed at something other than the real, local filesystem in
/// the future (the motivating example being a zip archive: browsable and
/// previewable the same way a directory tree is, without extracting it to
/// disk first). [`super::real::RealVfs`] is the only implementation
/// today, and is what `FsSource::new` uses by default; a hypothetical
/// zip-backed one would go through `FsSource::with_vfs` instead.
///
/// Every method takes a `Path` scoped by the caller (see
/// `FsSource::path_from_segments`) — a `Vfs` implementation has no
/// opinion of its own about a "root" or about which paths are valid; it
/// just answers questions about whatever path it's given, the same way
/// `std::fs`'s free functions do.
///
/// All methods but [`open`](Vfs::open) are synchronous and are always
/// called from inside a [`tokio::task::spawn_blocking`] closure by
/// `FsSource` (see `node_source::NodeSource`'s trait docs for why) — an
/// implementation doesn't need to worry about blocking the async runtime
/// itself. `open` is async because it returns a [`ByteStream`] the caller
/// reads from incrementally rather than a value computed once; see that
/// method's docs.
#[async_trait]
pub trait Vfs: Send + Sync {
    /// Resolves `path` to an absolute, symlink-free form, the way
    /// `std::fs::canonicalize` does. Used only once, by `FsSource::new`, to
    /// pin down the root a source is scoped to.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// Lists the immediate children of the directory at `path`. `cancelled`
    /// is threaded through so an implementation scanning a large directory
    /// entry-by-entry can bail out early once the result is known to be
    /// moot, the same way `super::FsSource::read_dir_sync`'s caller does
    /// for `std::fs::read_dir` today — see [`Cancelled`]'s docs; checking
    /// it is advisory, not required.
    fn read_dir(&self, path: &Path, cancelled: &Cancelled) -> io::Result<Vec<DirEntryInfo>>;

    /// Metadata for `path`, following a trailing symlink to describe its
    /// target (like `stat`/`std::fs::metadata`).
    fn metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// Metadata for `path` itself, not following a trailing symlink (like
    /// `lstat`/`std::fs::symlink_metadata`) — the only way to detect that
    /// `path` is a symlink at all, since `metadata` above resolves through
    /// it.
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// The target a symlink at `path` points to, unresolved (like
    /// `std::fs::read_link`). Only meaningful when `path` is a symlink.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    /// Reads up to `limit` bytes from the start of the file at `path`,
    /// however many are available (short of `limit` at EOF). Used for a
    /// bounded preview read, never the file's full contents — see
    /// `NodeSource::open`/[`Vfs::open`] for that.
    fn read_prefix(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>>;

    /// Opens `path` for streaming, unbounded access to its raw bytes,
    /// backing `NodeSource::open`. Async (unlike every other method here)
    /// because the caller wants to read the result incrementally rather
    /// than wait for it to be read into memory all at once — see
    /// [`ByteStream`]'s docs. `super::real::RealVfs`'s implementation
    /// is safe to call directly from async code (it's backed by
    /// `tokio::fs::File`, which already keeps blocking syscalls off the
    /// async runtime's own threads); an implementation backed by something
    /// that isn't already async-aware should do its own equivalent of
    /// `spawn_blocking` internally rather than block the caller's task.
    async fn open(&self, path: &Path) -> io::Result<ByteStream>;

    /// Like [`open`](Vfs::open), but guarantees the returned stream supports
    /// seeking, backing [`NodeSource::open_seekable`](crate::node_source::NodeSource::open_seekable).
    /// See that method's docs.
    async fn open_seekable(&self, path: &Path) -> io::Result<SeekableByteStream>;
}
