//! [`ZipVfs`]: the [`Vfs`] implementation this module exists for.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::runtime::Handle;

use crate::fs::vfs::{DirEntryInfo, Metadata, Vfs};
use crate::node_source::Cancelled;
use crate::streams::simulated_seeking::SimulatedSeek;
use crate::streams::{ByteStream, SeekableByteStream};

use super::bridge::SyncBridge;
use super::entry_stream::{Archive, EntryStream};
use super::index::{key_from_path, path_from_key, system_time_from_zip_datetime};
use super::index::{FileEntry, Index, Node};

/// A [`Vfs`] over the contents of a single zip archive, addressed the same
/// way a directory tree would be — `read_dir`/`metadata`/`open`/... all
/// operate on paths inside the archive rather than the real filesystem. See
/// the `zip` module's docs for why it's built over an arbitrary
/// [`SeekableByteStream`] rather than assuming the archive lives in a real
/// file: nothing here calls into `std::fs`.
///
/// Everything except entry *data* — [`Vfs::read_prefix`], [`Vfs::open`],
/// [`Vfs::open_seekable`] — is answered from an [`Index`] snapshotted once
/// at construction (see [`ZipVfs::new`]), so those calls touch neither the
/// underlying stream nor the `archive` lock at all.
pub struct ZipVfs {
    archive: Arc<Mutex<Archive>>,
    index: Index,
}

impl ZipVfs {
    /// Parses `stream`'s central directory and indexes every entry (see
    /// [`Index`]), so every later `Vfs` call that doesn't need an entry's
    /// actual bytes can answer purely from memory. Reads only what parsing
    /// the directory requires, plus — for each symlink entry — its (always
    /// small) target text; never `stream`'s full contents.
    pub async fn new(stream: SeekableByteStream) -> io::Result<Self> {
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            let bridge = SyncBridge::new(stream, handle);
            let mut archive = Archive::new(bridge).map_err(io::Error::from)?;
            let index = build_index(&mut archive)?;
            Ok(ZipVfs {
                archive: Arc::new(Mutex::new(archive)),
                index,
            })
        })
        .await
        .unwrap_or_else(|_| Err(io::Error::other("panicked while opening zip archive")))
    }

    fn resolve(&self, path: &Path, follow_final: bool) -> io::Result<(String, &Node)> {
        let key = key_from_path(path);
        self.index.resolve(&key, follow_final)
    }
}

/// Walks every entry once, recording its metadata into an [`Index`] —
/// see that type's docs for why this up-front pass lets almost every
/// `Vfs` method afterward be a plain, lock-free lookup.
fn build_index(archive: &mut Archive) -> io::Result<Index> {
    let mut index = Index::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(io::Error::from)?;
        let raw_name = file.name().to_string();
        let is_dir = file.is_dir();
        let is_symlink = file.is_symlink();
        let size = file.size();
        let modified = file
            .last_modified()
            .map(|dt| system_time_from_zip_datetime(&dt));

        // A symlink's "contents" are its target path text — always tiny,
        // unlike a regular file's data, so reading it fully here (rather
        // than lazily, like `read_prefix`/`open` do for everything else)
        // doesn't compromise the "index build never buffers real file
        // data" property. `from_utf8_lossy` rather than `read_to_string`
        // so one archive with a non-UTF-8 link target doesn't fail the
        // whole index.
        let symlink_target = if is_symlink {
            let mut raw = Vec::new();
            file.read_to_end(&mut raw)?;
            Some(PathBuf::from(String::from_utf8_lossy(&raw).into_owned()))
        } else {
            None
        };
        drop(file);

        let key = raw_name.trim_matches('/').to_string();
        if is_dir {
            index.insert_dir(key, modified);
        } else {
            index.insert_file(
                key,
                FileEntry {
                    archive_index: i,
                    size,
                    modified,
                    is_symlink,
                    symlink_target,
                },
            );
        }
    }
    Ok(index)
}

fn node_to_metadata(node: &Node) -> Metadata {
    match node {
        Node::Dir { modified, .. } => Metadata {
            is_dir: true,
            is_file: false,
            is_symlink: false,
            len: 0,
            modified: *modified,
            accessed: None,
            created: None,
            unix: None,
        },
        Node::File(f) => Metadata {
            is_dir: false,
            is_file: !f.is_symlink,
            is_symlink: f.is_symlink,
            len: f.size,
            modified: f.modified,
            accessed: None,
            created: None,
            unix: None,
        },
    }
}

/// Resolves `path` (following symlinks) to the regular-file entry it names,
/// rejecting a directory the same way opening one with `std::fs::File`
/// would.
fn resolve_file<'a>(zip: &'a ZipVfs, path: &Path) -> io::Result<&'a FileEntry> {
    let (key, node) = zip.resolve(path, true)?;
    match node {
        Node::File(f) => Ok(f),
        Node::Dir { .. } => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{key}: is a directory"),
        )),
    }
}

#[async_trait]
impl Vfs for ZipVfs {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let (key, _) = self.resolve(path, true)?;
        Ok(path_from_key(&key))
    }

    fn read_dir(&self, path: &Path, _cancelled: &Cancelled) -> io::Result<Vec<DirEntryInfo>> {
        let (key, node) = self.resolve(path, true)?;
        let children = match node {
            Node::Dir { children, .. } => children,
            Node::File(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{key}: not a directory"),
                ))
            }
        };
        Ok(children
            .iter()
            .map(|name| {
                let child_key = if key.is_empty() {
                    name.clone()
                } else {
                    format!("{key}/{name}")
                };
                let child = self
                    .index
                    .get(&child_key)
                    .expect("every listed child was linked in by insert_dir/insert_file");
                DirEntryInfo {
                    name: name.clone(),
                    is_dir: matches!(child, Node::Dir { .. }),
                    is_link: matches!(child, Node::File(f) if f.is_symlink),
                }
            })
            .collect())
    }

    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let (_, node) = self.resolve(path, true)?;
        Ok(node_to_metadata(node))
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        let (_, node) = self.resolve(path, false)?;
        Ok(node_to_metadata(node))
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let (key, node) = self.resolve(path, false)?;
        match node {
            Node::File(f) if f.is_symlink => Ok(f.symlink_target.clone().unwrap_or_default()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{key}: not a symlink"),
            )),
        }
    }

    fn read_prefix(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        let entry = resolve_file(self, path)?;
        let mut archive = self.archive.lock().unwrap();
        let reader = archive.by_index(entry.archive_index).map_err(io::Error::from)?;
        let mut buf = Vec::new();
        reader.take(limit as u64).read_to_end(&mut buf)?;
        Ok(buf)
    }

    async fn open(&self, path: &Path) -> io::Result<ByteStream> {
        let entry = resolve_file(self, path)?;
        let stream = EntryStream::new(self.archive.clone(), entry.archive_index);
        Ok(Box::pin(stream))
    }

    async fn open_seekable(&self, path: &Path) -> io::Result<SeekableByteStream> {
        let entry = resolve_file(self, path)?;
        let archive = self.archive.clone();
        let index = entry.archive_index;
        let make_stream: Box<dyn FnMut() -> ByteStream + Send> =
            Box::new(move || Box::pin(EntryStream::new(archive.clone(), index)) as ByteStream);
        Ok(Box::pin(SimulatedSeek::new(make_stream, entry.size)))
    }
}
