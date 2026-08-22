//! Ties `zip://` to `crate::fs`'s existing browsing/preview machinery
//! instead of reimplementing any of it: once [`super::ZipVfs`] has parsed
//! an archive's central directory, wrapping it in
//! [`crate::fs::FsSource::with_vfs`] gets everything `fs` already does for
//! free — directory-first sorting, syntax highlighting, the `hidden`/`raw`/
//! `meta` toggles, symlink handling — over the archive's contents instead
//! of the real filesystem. This module's only real job is producing that
//! `ZipVfs` in the first place, which needs the archive's bytes read from
//! wherever they actually live — see `construct_fn` below.
//!
//! `zip://` only makes sense piped from another node source (see
//! `crate::registry::create`'s pipe-parsing): `"file://archive.zip | zip://"`
//! opens the archive's root, `"file://archive.zip | zip://path/inside"`
//! opens a path within it. There's nothing for `zip://` to browse on its
//! own, so a bare `zip://` (no pipe) is rejected outright.

use std::io;
use std::sync::Arc;

use crate::fs::{self, FsSource};
use crate::node_source::{NodeSource, NodeSourceType};

use super::ZipVfs;

/// This type's contribution to [`crate::registry::NODE_SOURCE_TYPES`].
///
/// Its `commands`/`toggles`/toggle get-set functions are simply `fs`'s own
/// (see [`fs::NODE_SOURCE_TYPE`]) rather than a separate copy: what
/// `construct_fn` below actually hands back is an [`FsSource`], so browsing
/// a mounted archive supports exactly the same toggles (`hidden`, `raw`,
/// `meta`) as browsing a real directory, backed by the very same
/// process-global state `fs` already keeps for them — there's no
/// zip-specific toggle state to add here.
pub static NODE_SOURCE_TYPE: NodeSourceType = NodeSourceType {
    schemes: &["zip://"],
    manual_page: Some(&super::manual::MANUAL_PAGE),
    commands: fs::NODE_SOURCE_TYPE.commands,
    toggles: fs::NODE_SOURCE_TYPE.toggles,
    construct_fn: |_scheme, rest, pipe| {
        let rest = rest.to_string();
        Box::pin(async move {
            let Some(pipe) = pipe else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "zip:// has nothing to read on its own — pipe another node \
                     source's bytes into it, e.g. \"file://archive.zip | zip://\"",
                ));
            };
            // `open_seekable`, never `open`: parsing an archive's central
            // directory means seeking to its end first (see `ZipVfs::new`),
            // and faking that over a plain, forward-only `open` stream
            // would mean buffering the whole thing in memory first — which
            // would defeat the point for an archive too big to fit there.
            // A `pipe` that can't seek (e.g. something read from a socket)
            // fails this call outright instead, with whatever error it
            // gives for `open_seekable` itself.
            let stream = pipe.open_seekable(&[]).await?;
            let vfs = ZipVfs::new(stream).await?;
            Ok(Arc::new(FsSource::with_vfs(&rest, Arc::new(vfs))?) as Arc<dyn NodeSource>)
        })
    },
    set_toggle_fn: fs::NODE_SOURCE_TYPE.set_toggle_fn,
    get_toggle_fn: fs::NODE_SOURCE_TYPE.get_toggle_fn,
};

#[cfg(test)]
mod tests {
    use std::io::Write;

    use async_trait::async_trait;
    use zip::write::{SimpleFileOptions, ZipWriter};

    use crate::command::Command;
    use crate::entry::Entry;
    use crate::node_source::{Cancelled, Preview};
    use crate::streams::{ByteStream, SeekableByteStream};

    use super::*;

    /// Bytes for a small, real archive — built in memory (via a `Cursor`
    /// standing in for a file) so these tests never touch the real
    /// filesystem, unlike `crate::registry`'s own end-to-end pipe test
    /// (which does construct a `file://` source and so needs a real file).
    fn build_test_archive() -> Vec<u8> {
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.start_file("a.txt", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap().into_inner()
    }

    /// A `NodeSource` standing in for an upstream pipe that can't seek —
    /// e.g. something streamed straight off a socket. Every method other
    /// than `open_seekable` panics if called: `zip://`'s `construct_fn`
    /// should never reach for any of them, in particular never falling
    /// back to `open` and buffering the result itself (see
    /// `construct_rejects_a_pipe_that_cannot_seek_without_buffering_it`).
    struct UnseekablePipe;

    #[async_trait]
    impl NodeSource for UnseekablePipe {
        async fn read_dir(&self, _id: &[String]) -> io::Result<Vec<Entry>> {
            unreachable!("zip:// construction has no reason to list the pipe's entries")
        }

        async fn root_entry(&self) -> Entry {
            unreachable!("zip:// construction has no reason to read the pipe's root entry")
        }

        async fn preview_tui(&self, _id: &[String], _cancelled: &Cancelled) -> Preview {
            unreachable!("zip:// construction has no reason to preview the pipe")
        }

        async fn open(&self, _id: &[String]) -> io::Result<ByteStream> {
            unreachable!(
                "zip:// must fail via open_seekable, not fall back to buffering a plain open"
            )
        }

        async fn open_seekable(&self, _id: &[String]) -> io::Result<SeekableByteStream> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "cannot seek this pipe"))
        }

        async fn execute_command(&self, _command: &Command, _args: &[String]) -> io::Result<()> {
            unreachable!("zip:// construction has no reason to run a command on the pipe")
        }
    }

    /// A `NodeSource` whose `open_seekable` just hands back a fixed stream
    /// once — enough to stand in for an upstream pipe in these tests
    /// without needing a real `file://` source and a real file on disk (see
    /// `crate::registry`'s own end-to-end pipe test for one that does use a
    /// real file). Every other method is unreachable, same reasoning as
    /// [`UnseekablePipe`].
    struct CursorSource(tokio::sync::Mutex<Option<SeekableByteStream>>);

    impl CursorSource {
        fn new(stream: SeekableByteStream) -> Self {
            CursorSource(tokio::sync::Mutex::new(Some(stream)))
        }
    }

    #[async_trait]
    impl NodeSource for CursorSource {
        async fn read_dir(&self, _id: &[String]) -> io::Result<Vec<Entry>> {
            unreachable!("zip:// construction has no reason to list the pipe's entries")
        }

        async fn root_entry(&self) -> Entry {
            unreachable!("zip:// construction has no reason to read the pipe's root entry")
        }

        async fn preview_tui(&self, _id: &[String], _cancelled: &Cancelled) -> Preview {
            unreachable!("zip:// construction has no reason to preview the pipe")
        }

        async fn open(&self, _id: &[String]) -> io::Result<ByteStream> {
            unreachable!("zip:// must use open_seekable, not open")
        }

        async fn open_seekable(&self, _id: &[String]) -> io::Result<SeekableByteStream> {
            self.0
                .lock()
                .await
                .take()
                .ok_or_else(|| io::Error::other("CursorSource's stream was already taken"))
        }

        async fn execute_command(&self, _command: &Command, _args: &[String]) -> io::Result<()> {
            unreachable!("zip:// construction has no reason to run a command on the pipe")
        }
    }

    #[tokio::test]
    async fn construct_rejects_a_bare_zip_scheme_with_no_pipe() {
        let Err(err) = NODE_SOURCE_TYPE.construct("zip://", "", None).await else {
            panic!("expected zip:// with no pipe to fail");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn construct_rejects_a_pipe_that_cannot_seek_without_buffering_it() {
        let pipe: Arc<dyn NodeSource> = Arc::new(UnseekablePipe);
        let Err(err) = NODE_SOURCE_TYPE.construct("zip://", "", Some(pipe)).await else {
            panic!("expected a non-seekable pipe to fail construction");
        };
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn construct_mounts_a_piped_in_archive_at_its_root() {
        let stream: SeekableByteStream = Box::pin(std::io::Cursor::new(build_test_archive()));
        let pipe: Arc<dyn NodeSource> = Arc::new(CursorSource::new(stream));
        let source = NODE_SOURCE_TYPE
            .construct("zip://", "", Some(pipe))
            .await
            .expect("a valid archive should mount");
        let entries = source.read_dir(&[]).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a.txt"]);
    }

    #[tokio::test]
    async fn construct_fails_on_bytes_that_are_not_a_valid_archive() {
        let stream: SeekableByteStream = Box::pin(std::io::Cursor::new(b"not a zip file".to_vec()));
        let pipe: Arc<dyn NodeSource> = Arc::new(CursorSource::new(stream));
        let Err(err) = NODE_SOURCE_TYPE.construct("zip://", "", Some(pipe)).await else {
            panic!("expected garbage bytes to fail to parse as a zip archive");
        };
        assert_ne!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
