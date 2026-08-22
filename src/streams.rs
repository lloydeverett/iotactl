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
    use std::collections::VecDeque;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

    use super::ByteStream;

    const DISCARD_CHUNK: usize = 64 * 1024;

    /// An [`AsyncSeek`] wrapper over a `make_stream` factory that can
    /// produce a fresh, forward-only [`ByteStream`] starting at position 0.
    /// A forward seek is satisfied by discarding bytes through the current
    /// stream. A backward seek is satisfied for free when it lands within
    /// `buffer`, which holds the most recently streamed bytes (see
    /// `crate::config::slow_pipe_buffer_size`); otherwise it throws the
    /// current stream away and asks `make_stream` for a new one, then
    /// discards forward from there. A source that can only read
    /// sequentially, all the way from its start, each time it's opened —
    /// rather than truly at a caller-chosen offset — is exactly what this
    /// trades away: a seek that lands outside the buffer costs re-reading
    /// everything up to the target, not just jumping there.
    pub struct SimulatedSeek<F> {
        make_stream: F,
        inner: ByteStream,
        /// The most recently streamed bytes, covering the logical range
        /// `[stream_pos - buffer.len(), stream_pos)`. Bounded to
        /// `buffer_capacity`, trimming from the front as new bytes are
        /// appended at the back.
        buffer: VecDeque<u8>,
        buffer_capacity: usize,
        /// How far `inner` has actually been read (or discarded through)
        /// since it was last (re)created.
        stream_pos: u64,
        /// The logical position the caller sees. `<= stream_pos`, with any
        /// gap being replayed from `buffer` rather than read from `inner`.
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
        /// [`crate::config::allow_slow_pipes`]): a seek landing outside the
        /// lookback buffer means redoing whatever real work `make_stream`
        /// does from scratch (e.g. decompression), a cost a node source
        /// shouldn't impose on a caller without that caller opting in.
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
                buffer: VecDeque::new(),
                buffer_capacity: crate::config::slow_pipe_buffer_size(),
                stream_pos: 0,
                position: 0,
                total_len,
                seek_target: None,
            })
        }

        /// Appends `bytes` (just streamed from `inner`) to `buffer`,
        /// trimming from the front so it never holds more than
        /// `buffer_capacity` bytes.
        fn extend_buffer(&mut self, bytes: &[u8]) {
            if bytes.len() >= self.buffer_capacity {
                self.buffer.clear();
                self.buffer
                    .extend(&bytes[bytes.len() - self.buffer_capacity..]);
                return;
            }
            self.buffer.extend(bytes);
            let excess = self.buffer.len().saturating_sub(self.buffer_capacity);
            self.buffer.drain(..excess);
        }

        /// The earliest logical offset `buffer` currently covers.
        fn buffer_start(&self) -> u64 {
            self.stream_pos - self.buffer.len() as u64
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

            if this.position < this.stream_pos {
                // Replaying already-streamed bytes out of the buffer rather
                // than reading fresh ones from `inner`.
                let offset = (this.position - this.buffer_start()) as usize;
                let contiguous = this.buffer.make_contiguous();
                let available = &contiguous[offset..];
                let n = buf.remaining().min(available.len());
                buf.put_slice(&available[..n]);
                this.position += n as u64;
                return Poll::Ready(Ok(()));
            }

            let before = buf.filled().len();
            let result = this.inner.as_mut().poll_read(cx, buf);
            if result.is_ready() {
                let n = buf.filled().len() - before;
                this.extend_buffer(&buf.filled()[before..before + n]);
                this.position += n as u64;
                this.stream_pos += n as u64;
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

            if target < this.buffer_start() {
                // Outside what's buffered: only a fresh stream, restarted
                // from scratch, can reach it.
                this.inner = (this.make_stream)();
                this.buffer.clear();
                this.stream_pos = 0;
            }

            if target <= this.stream_pos {
                // Already streamed (and still buffered): satisfied without
                // touching `inner` at all.
                this.position = target;
                this.seek_target = None;
                return Poll::Ready(Ok(this.position));
            }

            this.position = this.stream_pos;
            let mut discard = [0u8; DISCARD_CHUNK];
            while this.stream_pos < target {
                let want = ((target - this.stream_pos).min(DISCARD_CHUNK as u64)) as usize;
                let mut read_buf = ReadBuf::new(&mut discard[..want]);
                match this.inner.as_mut().poll_read(cx, &mut read_buf) {
                    Poll::Ready(Ok(())) => {
                        let n = read_buf.filled().len();
                        if n == 0 {
                            break; // EOF short of the target; clamp to where we got
                        }
                        this.extend_buffer(&discard[..n]);
                        this.stream_pos += n as u64;
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

    #[cfg(test)]
    mod tests {
        use std::io::Cursor;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        use super::{ByteStream, SimulatedSeek};

        /// A `make_stream` factory that counts how many times it's called —
        /// i.e. how many times the underlying stream was (re)started from
        /// scratch — alongside handing back `data` itself each time.
        fn counting_stream(
            data: &'static [u8],
            calls: Arc<AtomicUsize>,
        ) -> impl FnMut() -> ByteStream {
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(Cursor::new(data)) as ByteStream
            }
        }

        #[tokio::test]
        async fn seek_within_the_buffer_replays_instead_of_restarting() {
            crate::config::ensure_initialized_for_tests();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut stream = SimulatedSeek::new(counting_stream(b"hello", calls.clone()), 5)
                .expect("--allow-slow-pipes is on for tests");

            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hello");
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // The test buffer is 2 bytes (see
            // `config::ensure_initialized_for_tests`), so the last 2 bytes
            // read ("lo") are still in it: seeking back into them shouldn't
            // touch `make_stream` again.
            stream.seek(std::io::SeekFrom::Start(4)).await.unwrap();
            let mut one = [0u8; 1];
            stream.read_exact(&mut one).await.unwrap();
            assert_eq!(&one, b"o");
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn seek_outside_the_buffer_restarts_the_stream() {
            crate::config::ensure_initialized_for_tests();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut stream = SimulatedSeek::new(counting_stream(b"hello", calls.clone()), 5)
                .expect("--allow-slow-pipes is on for tests");

            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // The 2-byte test buffer only covers the last 2 bytes read;
            // seeking all the way back to 0 lands outside it.
            stream.seek(std::io::SeekFrom::Start(0)).await.unwrap();
            let mut buf2 = [0u8; 5];
            stream.read_exact(&mut buf2).await.unwrap();
            assert_eq!(&buf2, b"hello");
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        #[tokio::test]
        async fn a_zero_size_buffer_restarts_on_every_backward_seek() {
            crate::config::ensure_initialized_for_tests();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut stream = SimulatedSeek::new(counting_stream(b"hello", calls.clone()), 5)
                .expect("--allow-slow-pipes is on for tests");
            stream.buffer_capacity = 0;

            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // Even seeking back by a single byte has nothing buffered to
            // replay from, so it must restart.
            stream.seek(std::io::SeekFrom::Start(4)).await.unwrap();
            let mut one = [0u8; 1];
            stream.read_exact(&mut one).await.unwrap();
            assert_eq!(&one, b"o");
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }
}
