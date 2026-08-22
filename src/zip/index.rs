//! An in-memory snapshot of an archive's directory structure, built once
//! from its central directory (see `archive::build_index`) so almost every
//! [`crate::fs::vfs::Vfs`] method can answer purely from memory afterward —
//! see [`Index`]'s docs.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Maximum symlink hops [`Index::resolve`] will follow before giving up,
/// mirroring the ELOOP threshold real filesystems use (Linux's is 40).
const MAX_SYMLINK_HOPS: u32 = 40;

#[derive(Debug)]
pub(super) struct FileEntry {
    /// Index into the archive's central directory (`zip::ZipArchive::
    /// by_index`), used to seek straight to this entry's data when its
    /// bytes are actually needed.
    pub(super) archive_index: usize,
    /// Uncompressed size — for a symlink entry, the length of its target
    /// path text, matching what `lstat`'s `st_size` reports on a real one.
    pub(super) size: u64,
    pub(super) modified: Option<SystemTime>,
    pub(super) is_symlink: bool,
    /// `Some` exactly when `is_symlink`, populated at index-build time by
    /// reading this (always small) entry's decompressed content — see
    /// `archive::build_index`.
    pub(super) symlink_target: Option<PathBuf>,
}

#[derive(Debug)]
pub(super) enum Node {
    Dir {
        /// Immediate child names (not full paths) — `Index::resolve` and
        /// `read_dir` rejoin these with the parent's own resolved key on
        /// demand rather than storing full paths redundantly here.
        children: BTreeSet<String>,
        /// `Some` only when the archive has an explicit directory entry for
        /// this path; zip archives commonly omit those, inferring
        /// directories instead from file paths, in which case there's no
        /// metadata for it to carry.
        modified: Option<SystemTime>,
    },
    File(FileEntry),
}

/// A path-keyed snapshot of an archive's structure. Keyed by a normalized,
/// `/`-joined path with no leading or trailing slash (the root is the empty
/// string `""`), matching zip's own entry naming convention.
///
/// Built once up front (see `archive::build_index`) so every `Vfs` method
/// except the ones that read an entry's actual bytes — `read_prefix`,
/// `open`, `open_seekable` — is a pure lookup here, needing no lock on the
/// underlying archive/stream at all.
#[derive(Debug)]
pub(super) struct Index {
    nodes: BTreeMap<String, Node>,
}

impl Index {
    pub(super) fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            String::new(),
            Node::Dir {
                children: BTreeSet::new(),
                modified: None,
            },
        );
        Index { nodes }
    }

    /// Registers a directory entry at `key`, auto-vivifying (and linking
    /// into their own parent) any ancestor directories not yet seen.
    /// Preserves any children already recorded under `key` from an earlier,
    /// implicit auto-vivification (a file entry seen before this archive's
    /// own explicit entry for the same directory).
    pub(super) fn insert_dir(&mut self, key: String, modified: Option<SystemTime>) {
        let children = match self.nodes.remove(&key) {
            Some(Node::Dir { children, .. }) => children,
            _ => BTreeSet::new(),
        };
        self.nodes.insert(
            key.clone(),
            Node::Dir {
                children,
                modified,
            },
        );
        self.register_in_parent(&key);
    }

    pub(super) fn insert_file(&mut self, key: String, entry: FileEntry) {
        self.nodes.insert(key.clone(), Node::File(entry));
        self.register_in_parent(&key);
    }

    /// Ensures `key`'s parent chain exists (auto-vivifying implicit
    /// directories as needed) and that `key`'s own name is listed as a
    /// child of its immediate parent.
    fn register_in_parent(&mut self, key: &str) {
        if key.is_empty() {
            return; // the root has no parent to register into
        }
        let (parent, name) = match key.rfind('/') {
            Some(idx) => (&key[..idx], &key[idx + 1..]),
            None => ("", key),
        };
        self.ensure_dir(parent);
        if let Some(Node::Dir { children, .. }) = self.nodes.get_mut(parent) {
            children.insert(name.to_string());
        }
    }

    fn ensure_dir(&mut self, key: &str) {
        if self.nodes.contains_key(key) {
            return;
        }
        self.nodes.insert(
            key.to_string(),
            Node::Dir {
                children: BTreeSet::new(),
                modified: None,
            },
        );
        self.register_in_parent(key);
    }

    pub(super) fn get(&self, key: &str) -> Option<&Node> {
        self.nodes.get(key)
    }

    /// Walks `key` component by component, resolving any symlink
    /// encountered mid-path unconditionally, and the final component only
    /// when `follow_final` is set — the `stat`/`lstat` distinction
    /// `Vfs::metadata`/`Vfs::symlink_metadata` need. Returns the
    /// fully-resolved key alongside the node it names.
    pub(super) fn resolve(&self, key: &str, follow_final: bool) -> io::Result<(String, &Node)> {
        let mut resolved = String::new();
        let mut queue: VecDeque<String> = split_normal(key).collect();
        let mut hops = 0u32;

        while let Some(name) = queue.pop_front() {
            if name == "." {
                continue;
            }
            if name == ".." {
                if let Some(idx) = resolved.rfind('/') {
                    resolved.truncate(idx);
                } else {
                    resolved.clear();
                }
                continue;
            }

            let candidate = join(&resolved, &name);
            let node = self
                .nodes
                .get(&candidate)
                .ok_or_else(|| not_found(&candidate))?;
            let is_last = queue.is_empty();

            match node {
                Node::File(f) if f.is_symlink && (!is_last || follow_final) => {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "too many levels of symbolic links",
                        ));
                    }
                    let target = f
                        .symlink_target
                        .as_deref()
                        .unwrap_or(Path::new(""))
                        .to_string_lossy()
                        .into_owned();
                    if target.starts_with('/') {
                        resolved.clear();
                    }
                    for part in target.split('/').filter(|s| !s.is_empty()).rev() {
                        queue.push_front(part.to_string());
                    }
                }
                Node::File(_) if !is_last => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("{candidate}: not a directory"),
                    ));
                }
                _ => resolved = candidate,
            }
        }

        let node = self
            .nodes
            .get(&resolved)
            .ok_or_else(|| not_found(&resolved))?;
        Ok((resolved, node))
    }
}

fn not_found(key: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{key}: not found in archive"),
    )
}

fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

/// Splits an already `/`-joined key back into its plain components, used
/// only to seed [`Index::resolve`]'s work queue — its own `.`/`..` handling
/// applies uniformly to the original path and to any symlink target
/// spliced in along the way, so no normalization happens here.
fn split_normal(key: &str) -> impl Iterator<Item = String> + '_ {
    key.split('/').filter(|s| !s.is_empty()).map(String::from)
}

/// Normalizes an OS [`Path`] (as `Vfs` callers hand in) to this module's
/// `/`-joined key form. `.`/`..` are collapsed structurally here since they
/// never depend on archive contents — that only matters for a symlink
/// target's own components, handled separately inside [`Index::resolve`].
pub(super) fn key_from_path(path: &Path) -> String {
    let mut stack: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(s) => stack.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => {
                stack.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    stack.join("/")
}

/// Inverse of [`key_from_path`]: this module's own root (`""`) becomes `/`,
/// with each component pushed back on as a plain segment.
pub(super) fn path_from_key(key: &str) -> PathBuf {
    let mut path = PathBuf::from("/");
    for part in key.split('/').filter(|s| !s.is_empty()) {
        path.push(part);
    }
    path
}

/// Zip's DOS-derived timestamp carries no timezone, so — like `fs::
/// format_time`'s handling of a real, already-UTC `SystemTime` — this just
/// treats the components as UTC.
pub(super) fn system_time_from_zip_datetime(dt: &zip::DateTime) -> SystemTime {
    let days = days_from_civil(dt.year() as i64, dt.month() as u32, dt.day() as u32);
    let secs =
        days * 86400 + dt.hour() as i64 * 3600 + dt.minute() as i64 * 60 + dt.second() as i64;
    if secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs((-secs) as u64)
    }
}

/// Inverse of `fs::civil_from_days` (Howard Hinnant's algorithm again,
/// <http://howardhinnant.github.io/date_algorithms.html>): days since the
/// Unix epoch for a given civil (year, month, day).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let mp = (if m > 2 { m - 3 } else { m + 9 }) as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}
