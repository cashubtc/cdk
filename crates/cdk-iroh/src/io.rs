use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
};

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A safe Tokio byte stream composed from Iroh's receive and send halves.
///
/// Hyper and Tokio Tungstenite require one value implementing both
/// [`AsyncRead`] and [`AsyncWrite`]. Iroh exposes the two directions
/// separately, so this adapter delegates each operation to the corresponding
/// half without unsafe pin projection or buffering.
pub struct IrohStream {
    recv: RecvStream,
    send: SendStream,
    finished: bool,
}

impl IrohStream {
    /// Combines the send and receive halves of one bidirectional Iroh stream.
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self {
            recv,
            send,
            finished: false,
        }
    }
}

impl fmt::Debug for IrohStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrohStream")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for IrohStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(context, buffer)
    }
}

impl AsyncWrite for IrohStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.send), context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.finished {
            return Poll::Ready(Ok(()));
        }
        match Pin::new(&mut self.send).poll_flush(context) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => return Poll::Pending,
        }
        match self.send.finish() {
            Ok(()) => {
                self.finished = true;
                Poll::Ready(Ok(()))
            }
            Err(error) => Poll::Ready(Err(io::Error::other(error.to_string()))),
        }
    }
}
