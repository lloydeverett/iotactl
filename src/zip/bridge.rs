//! Adapts an async, seekable byte stream into the synchronous `Read + Seek`
//! the `zip` crate requires.

use std::io::{self, Read, Seek, SeekFrom};

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::runtime::Handle;

use crate::streams::SeekableByteStream;

/// Wraps a [`SeekableByteStream`] so it can back a `zip::ZipArchive`, by
/// blocking on each read/seek via `Handle::block_on`. This is what lets
/// [`super::ZipVfs`] be built over any seekable async byte source — a local
/// file, something read from a network connection, another `Vfs`'s own
/// `open_seekable` — rather than requiring the archive to already live on
/// the real, local filesystem.
///
/// Only ever driven from inside a `tokio::task::spawn_blocking` closure
/// (see `archive.rs` and `entry_stream.rs`): `Handle::block_on` panics if
/// called from a thread the async runtime is actively polling tasks on,
/// but is exactly the documented way to bridge a sync-only library from a
/// dedicated blocking-pool thread, which every caller here already is.
pub(super) struct SyncBridge {
    inner: SeekableByteStream,
    handle: Handle,
}

impl SyncBridge {
    pub(super) fn new(inner: SeekableByteStream, handle: Handle) -> Self {
        SyncBridge { inner, handle }
    }
}

impl Read for SyncBridge {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.handle.block_on(self.inner.read(buf))
    }
}

impl Seek for SyncBridge {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.handle.block_on(self.inner.seek(pos))
    }
}
