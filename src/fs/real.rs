//! The one [`Vfs`] implementation that exists today: every method
//! backed by a real `std::fs`/`tokio::fs` call against the local
//! filesystem. Kept separate from `vfs`'s trait definition, and from
//! `super`'s browsing/preview logic, so both stay entirely free of real
//! I/O — see `vfs`'s module docs for why that separation is the point.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::node_source::{ByteStream, Cancelled};

use super::vfs::{DirEntryInfo, Vfs, Metadata, UnixMetadata};

/// See the module docs.
pub struct RealVfs;

/// Converts a real `std::fs::Metadata` into this module's backend-agnostic
/// [`Metadata`]. Shared by [`Vfs::metadata`] and
/// [`Vfs::symlink_metadata`] below, which differ only in whether the
/// `std::fs::Metadata` they hand in already followed a symlink.
fn convert_metadata(meta: fs::Metadata) -> Metadata {
    #[cfg(unix)]
    let unix = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        Some(UnixMetadata {
            mode: meta.permissions().mode(),
            uid: meta.uid(),
            gid: meta.gid(),
        })
    };
    #[cfg(not(unix))]
    let unix = None;

    Metadata {
        is_dir: meta.is_dir(),
        is_file: meta.is_file(),
        is_symlink: meta.file_type().is_symlink(),
        len: meta.len(),
        modified: meta.modified().ok(),
        accessed: meta.accessed().ok(),
        created: meta.created().ok(),
        unix,
    }
}

#[async_trait]
impl Vfs for RealVfs {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn read_dir(&self, path: &Path, cancelled: &Cancelled) -> io::Result<Vec<DirEntryInfo>> {
        let mut entries = Vec::new();
        for res in fs::read_dir(path)? {
            if cancelled.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let dir_entry = res?;
            let name = dir_entry.file_name().to_string_lossy().to_string();

            // DirEntry::metadata does not follow symlinks, so we can detect
            // them and then resolve the target separately to know if it's a
            // directory.
            let link_metadata = dir_entry.metadata()?;
            let is_link = link_metadata.file_type().is_symlink();
            let is_dir = if is_link {
                fs::metadata(dir_entry.path())
                    .map(|target_meta| target_meta.is_dir())
                    .unwrap_or(false)
            } else {
                link_metadata.is_dir()
            };

            entries.push(DirEntryInfo {
                name,
                is_dir,
                is_link,
            });
        }
        Ok(entries)
    }

    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        fs::metadata(path).map(convert_metadata)
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        fs::symlink_metadata(path).map(convert_metadata)
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn read_prefix(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        let mut file = fs::File::open(path)?;
        let mut buf = vec![0u8; limit];
        let n = file.read(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn open(&self, path: &Path) -> io::Result<ByteStream> {
        // `tokio::fs::File::open` already runs the actual (blocking) open
        // syscall via `spawn_blocking` internally, and its `AsyncRead` impl
        // does the same per-read, so no explicit `spawn_blocking` is needed
        // here.
        let file = tokio::fs::File::open(path).await?;
        Ok(Box::pin(file))
    }
}
