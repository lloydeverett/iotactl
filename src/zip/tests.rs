use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Arc;

use zip::write::{SimpleFileOptions, ZipWriter};

use crate::fs::vfs::Vfs;
use crate::streams::SeekableByteStream;

use super::ZipVfs;

fn build_test_archive() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));

    writer.start_file("a.txt", SimpleFileOptions::default()).unwrap();
    writer.write_all(b"hello").unwrap();

    writer
        .start_file("dir/b.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"world").unwrap();

    writer
        .start_file("dir/sub/c.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"!").unwrap();

    writer
        .add_symlink("link.txt", "a.txt", SimpleFileOptions::default())
        .unwrap();
    writer
        .add_symlink("broken.txt", "missing.txt", SimpleFileOptions::default())
        .unwrap();

    writer.finish().unwrap().into_inner()
}

async fn open_test_vfs() -> ZipVfs {
    let bytes = build_test_archive();
    let stream: SeekableByteStream = Box::pin(Cursor::new(bytes));
    ZipVfs::new(stream).await.expect("test archive should parse")
}

#[tokio::test]
async fn root_lists_top_level_entries() {
    let vfs = open_test_vfs().await;
    let mut names: Vec<_> = vfs
        .read_dir(Path::new("/"), &Default::default())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.txt", "broken.txt", "dir", "link.txt"]);
}

#[tokio::test]
async fn nested_directory_lists_correctly() {
    let vfs = open_test_vfs().await;
    let entries = vfs.read_dir(Path::new("/dir"), &Default::default()).unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["b.txt", "sub"]);
    assert!(entries.iter().find(|e| e.name == "sub").unwrap().is_dir);
    assert!(!entries.iter().find(|e| e.name == "b.txt").unwrap().is_dir);
}

#[tokio::test]
async fn metadata_reports_file_size() {
    let vfs = open_test_vfs().await;
    let meta = vfs.metadata(Path::new("/a.txt")).unwrap();
    assert!(meta.is_file);
    assert_eq!(meta.len, 5);
}

#[tokio::test]
async fn symlink_metadata_vs_metadata_follow() {
    let vfs = open_test_vfs().await;

    let link_meta = vfs.symlink_metadata(Path::new("/link.txt")).unwrap();
    assert!(link_meta.is_symlink);
    assert!(!link_meta.is_file);

    let followed = vfs.metadata(Path::new("/link.txt")).unwrap();
    assert!(followed.is_file);
    assert!(!followed.is_symlink);
    assert_eq!(followed.len, 5);
}

#[tokio::test]
async fn read_link_returns_raw_target() {
    let vfs = open_test_vfs().await;
    let target = vfs.read_link(Path::new("/link.txt")).unwrap();
    assert_eq!(target, Path::new("a.txt"));
}

#[tokio::test]
async fn broken_symlink_metadata_errors_but_lstat_succeeds() {
    let vfs = open_test_vfs().await;
    assert!(vfs.metadata(Path::new("/broken.txt")).is_err());
    assert!(vfs.symlink_metadata(Path::new("/broken.txt")).is_ok());
}

#[tokio::test]
async fn read_prefix_respects_limit() {
    // `read_prefix` locks the archive and drives `SyncBridge`'s
    // `Handle::block_on`, so — like every real caller (see `FsSource::
    // read_dir_sync` and friends) — it must run inside `spawn_blocking`
    // rather than directly on an async-task thread.
    let vfs = Arc::new(open_test_vfs().await);
    let buf = tokio::task::spawn_blocking(move || vfs.read_prefix(Path::new("/a.txt"), 3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(buf, b"hel");
}

#[tokio::test]
async fn open_streams_full_contents() {
    let vfs = open_test_vfs().await;
    let mut stream = vfs.open(Path::new("/dir/b.txt")).await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, b"world");
}

#[tokio::test]
async fn open_seekable_can_seek_forward_and_backward() {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let vfs = open_test_vfs().await;
    let mut stream = vfs.open_seekable(Path::new("/a.txt")).await.unwrap();

    stream.seek(std::io::SeekFrom::Start(2)).await.unwrap();
    let mut buf = [0u8; 3];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"llo");

    // Seeking backward forces the entry stream to restart from scratch.
    stream.seek(std::io::SeekFrom::Start(0)).await.unwrap();
    let mut buf2 = [0u8; 5];
    stream.read_exact(&mut buf2).await.unwrap();
    assert_eq!(&buf2, b"hello");
}

#[tokio::test]
async fn canonicalize_resolves_dot_dot_and_symlinks() {
    let vfs = open_test_vfs().await;
    let resolved = vfs.canonicalize(Path::new("/dir/../link.txt")).unwrap();
    assert_eq!(resolved, Path::new("/a.txt"));
}
