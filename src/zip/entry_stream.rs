//! An `AsyncRead` stream over one archive entry's decompressed bytes,
//! backing [`super::ZipVfs::open`]. The `zip` crate's decompression is
//! entirely synchronous, so it reads on a dedicated `spawn_blocking` thread
//! and hands chunks back across a channel — the same shape
//! [`super::bridge::SyncBridge`] uses for the opposite direction
//! (sync-over-async there, async-over-sync here).
//!
//! [`super::ZipVfs::open_seekable`] wraps this in
//! [`crate::streams::simulated_seeking::SimulatedSeek`] instead of its own
//! seekable variant: the `zip` crate gives no random access into a
//! compressed entry's decompressed bytes (true even of
//! `zip::ZipArchive::by_index_seek`, which only helps the `Stored` method),
//! so a seek here can only ever mean discarding forward through a fresh
//! [`EntryStream`], which is exactly what that wrapper already does for any
//! stream shaped this way.

use std::io::{self, Read};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
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

/// A [`crate::streams::ByteStream`] over one archive entry's decompressed
/// bytes. Forward-only, from the entry's start — see this module's doc
/// comment for how [`super::ZipVfs::open_seekable`] gets seeking out of
/// repeated, fresh instances of this type.
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
}

impl AsyncRead for EntryStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.pending_pos < this.pending.len() {
                let n = buf.remaining().min(this.pending.len() - this.pending_pos);
                buf.put_slice(&this.pending[this.pending_pos..this.pending_pos + n]);
                this.pending_pos += n;
                return Poll::Ready(Ok(()));
            }
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.pending = chunk;
                    this.pending_pos = 0;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF: 0 bytes filled
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
