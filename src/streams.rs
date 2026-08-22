//! Byte-stream types a [`crate::node_source::NodeSource`] hands back from
//! `open`/`open_seekable`, plus [`simulated_seeking`]: a facility any node
//! source can use to offer seeking over a stream that can otherwise only be
//! read forward, once, from the start.

use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncSeek};

/// A boxed, `Send` handle to a node's raw, unrendered byte content, returned
/// by [`crate::node_source::NodeSource::open`]. Unlike a preview — which
/// trades faithfulness for a bounded, display-ready result (a size limit,
/// binary files skipped, markdown rendered) — this always yields the
/// underlying bytes exactly, however large or unprintable the node is. It's
/// a stream rather than an owned buffer so a caller can consume those bytes
/// incrementally instead of holding the whole thing in memory at once.
pub type ByteStream = Pin<Box<dyn AsyncRead + Send>>;

/// A reader that also supports seeking, so it can be boxed as a single
/// trait object rather than two separate ones over the same underlying
/// value.
pub trait AsyncReadSeek: AsyncRead + AsyncSeek {}
impl<T: AsyncRead + AsyncSeek + ?Sized> AsyncReadSeek for T {}

/// Like [`ByteStream`], but returned by
/// [`crate::node_source::NodeSource::open_seekable`]: whenever that call
/// succeeds, the stream it hands back is guaranteed to support seeking,
/// unlike a plain [`ByteStream`] which may or may not.
pub type SeekableByteStream = Pin<Box<dyn AsyncReadSeek + Send>>;

/// Fakes seeking over a stream that's naturally only readable forward, once,
/// from its start — the shape of e.g. a compressed archive entry, whose
/// format gives no random access into its decompressed bytes.
pub mod simulated_seeking {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

    use super::ByteStream;

    const DISCARD_CHUNK: usize = 64 * 1024;

    /// An [`AsyncSeek`] wrapper over a `make_stream` factory that can
    /// produce a fresh, forward-only [`ByteStream`] starting at position 0.
    /// A forward seek is satisfied by discarding bytes through the current
    /// stream; a backward seek throws the current stream away and asks
    /// `make_stream` for a new one, then discards forward from there. A
    /// source that can only read sequentially, all the way from its start,
    /// each time it's opened — rather than truly at a caller-chosen offset —
    /// is exactly what this trades away: seeking backward costs re-reading
    /// everything up to the target, not just jumping there.
    pub struct SimulatedSeek<F> {
        make_stream: F,
        inner: ByteStream,
        /// Absolute offset into the stream already handed to the caller (or
        /// discarded while seeking).
        position: u64,
        total_len: u64,
        seek_target: Option<u64>,
    }

    impl<F> SimulatedSeek<F>
    where
        F: FnMut() -> ByteStream,
    {
        /// `make_stream` must return a fresh stream reading from position 0
        /// each time it's called. `total_len` is the stream's full length,
        /// used to resolve `SeekFrom::End`.
        ///
        /// Refuses to construct unless `--allow-slow-pipes` was passed (see
        /// [`crate::config::allow_slow_pipes`]): a seek backward here means
        /// redoing whatever real work `make_stream` does from scratch (e.g.
        /// decompression), a cost a node source shouldn't impose on a
        /// caller without that caller opting in.
        pub fn new(mut make_stream: F, total_len: u64) -> io::Result<Self> {
            if !crate::config::allow_slow_pipes() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "construction requires random access but underlying stream is not \
                     random access (see --allow-slow-pipes to permit simulated seeking)",
                ));
            }
            let inner = make_stream();
            Ok(SimulatedSeek {
                make_stream,
                inner,
                position: 0,
                total_len,
                seek_target: None,
            })
        }
    }

    impl<F> AsyncRead for SimulatedSeek<F>
    where
        F: FnMut() -> ByteStream + Unpin,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let before = buf.filled().len();
            let result = this.inner.as_mut().poll_read(cx, buf);
            if result.is_ready() {
                this.position += (buf.filled().len() - before) as u64;
            }
            result
        }
    }

    impl<F> AsyncSeek for SimulatedSeek<F>
    where
        F: FnMut() -> ByteStream + Unpin,
    {
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
                this.inner = (this.make_stream)();
                this.position = 0;
            }

            let mut discard = [0u8; DISCARD_CHUNK];
            while this.position < target {
                let want = ((target - this.position).min(DISCARD_CHUNK as u64)) as usize;
                let mut read_buf = ReadBuf::new(&mut discard[..want]);
                match this.inner.as_mut().poll_read(cx, &mut read_buf) {
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
}
