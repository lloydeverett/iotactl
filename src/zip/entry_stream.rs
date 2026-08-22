//! `AsyncRead`/`AsyncSeek` streams over one archive entry's decompressed
//! bytes, backing [`super::ZipVfs::open`]/[`super::ZipVfs::open_seekable`].
//! The `zip` crate's decompression is entirely synchronous, so both types
//! here read on a dedicated `spawn_blocking` thread and hand chunks back
//! across a channel — the same shape [`super::bridge::SyncBridge`] uses for
//! the opposite direction (sync-over-async there, async-over-sync here).

use std::io::{self, Read};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio::sync::mpsc;

use super::bridge::SyncBridge;

pub(super) type Archive = zip::ZipArchive<SyncBridge>;

const CHUNK_SIZE: usize = 64 * 1024;
const CHANNEL_CAPACITY: usize = 4;

/// Reads one entry's decompressed bytes on a dedicated blocking thread,
/// sending chunks to the async side through a bounded channel (bounded so a
/// slow reader applies backpressure instead of this task decompressing the
/// whole entry into memory ahead of demand). Holds `archive`'s lock for as
/// long as it runs, which serializes it against every other call that
/// touches the underlying stream — `read_dir`/`metadata`/... don't, since
/// those only consult the in-memory `Index` (see that module's docs) —
/// since there is, underneath it all, exactly one shared reader.
fn spawn_entry_pump(
    archive: Arc<Mutex<Archive>>,
    index: usize,
) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::task::spawn_blocking(move || {
        let mut guard = match archive.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let mut reader = match guard.by_index(index) {
            Ok(reader) => reader,
            Err(e) => {
                let _ = tx.blocking_send(Err(io::Error::from(e)));
                return;
            }
        };
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                        break; // the reader side went away; no one wants more
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });
    rx
}

/// A [`crate::node_source::ByteStream`] over one archive entry's
/// decompressed bytes. Forward-only — see [`SeekableEntryStream`] for the
/// seekable variant `open_seekable` needs.
pub(super) struct EntryStream {
    rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    pending_pos: usize,
}

impl EntryStream {
    pub(super) fn new(archive: Arc<Mutex<Archive>>, index: usize) -> Self {
        EntryStream {
            rx: spawn_entry_pump(archive, index),
            pending: Vec::new(),
            pending_pos: 0,
        }
    }

    /// Shared by both this type's `AsyncRead` impl and
    /// [`SeekableEntryStream`]'s: fills as much of `buf` as one
    /// already-received chunk (or the next one polled from `rx`) can
    /// satisfy.
    fn poll_fill(&mut self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.pending_pos < self.pending.len() {
                let n = buf.remaining().min(self.pending.len() - self.pending_pos);
                buf.put_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
                self.pending_pos += n;
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.pending = chunk;
                    self.pending_pos = 0;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF: 0 bytes filled
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncRead for EntryStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().poll_fill(cx, buf)
    }
}

/// Like [`EntryStream`], but also implements `AsyncSeek`. The `zip` crate
/// gives no random access into a compressed entry's decompressed bytes
/// (true even of `zip::ZipArchive::by_index_seek`, which only helps the
/// `Stored` method), so a seek that can't be satisfied by discarding
/// forward through the current pump instead throws it away and starts a
/// fresh one from the entry's beginning. That's the trade-off inherent in
/// asking for a *seekable* stream over what's actually a compressed,
/// sequential format, rather than reading it once, straight through.
pub(super) struct SeekableEntryStream {
    archive: Arc<Mutex<Archive>>,
    index: usize,
    inner: EntryStream,
    /// Absolute offset into the entry's decompressed bytes already handed
    /// to the caller (or discarded while seeking).
    position: u64,
    total_len: u64,
    seek_target: Option<u64>,
}

impl SeekableEntryStream {
    pub(super) fn new(archive: Arc<Mutex<Archive>>, index: usize, total_len: u64) -> Self {
        let inner = EntryStream::new(archive.clone(), index);
        SeekableEntryStream {
            archive,
            index,
            inner,
            position: 0,
            total_len,
            seek_target: None,
        }
    }
}

impl AsyncRead for SeekableEntryStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = this.inner.poll_fill(cx, buf);
        if result.is_ready() {
            this.position += (buf.filled().len() - before) as u64;
        }
        result
    }
}

impl AsyncSeek for SeekableEntryStream {
    fn start_seek(self: Pin<&mut Self>, position: io::SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        let target = match position {
            io::SeekFrom::Start(n) => n as i128,
            io::SeekFrom::End(n) => this.total_len as i128 + n as i128,
            io::SeekFrom::Current(n) => this.position as i128 + n as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative position",
            ));
        }
        this.seek_target = Some(target.min(this.total_len as i128) as u64);
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let this = self.get_mut();
        let Some(target) = this.seek_target else {
            return Poll::Ready(Ok(this.position));
        };

        if target < this.position {
            this.inner = EntryStream::new(this.archive.clone(), this.index);
            this.position = 0;
        }

        let mut discard = [0u8; CHUNK_SIZE];
        while this.position < target {
            let want = ((target - this.position).min(CHUNK_SIZE as u64)) as usize;
            let mut read_buf = ReadBuf::new(&mut discard[..want]);
            match this.inner.poll_fill(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    let n = read_buf.filled().len();
                    if n == 0 {
                        break; // EOF short of the target; clamp to where we got
                    }
                    this.position += n as u64;
                }
                Poll::Ready(Err(e)) => {
                    this.seek_target = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        this.seek_target = None;
        Poll::Ready(Ok(this.position))
    }
}
