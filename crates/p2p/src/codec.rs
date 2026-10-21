//! Length-prefixed binary codec for the direct-message protocol.
//!
//! Frame: `u32 big-endian length | bytes`. Sizes are capped to bound memory use.

use std::io;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::StreamProtocol;
use libp2p::request_response::Codec;

use crate::{MAX_PAYLOAD, MAX_REPLY};

/// Protocol name of direct messages.
pub(crate) const DM_PROTOCOL: StreamProtocol = StreamProtocol::new("/outpostwave/dm/1");

/// Codec carrying opaque byte payloads.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BytesCodec;

async fn read_frame<T: AsyncRead + Unpin + Send>(io: &mut T, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    io.read_exact(&mut len).await?;
    let len = usize::try_from(u32::from_be_bytes(len))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame<T: AsyncWrite + Unpin + Send>(
    io: &mut T,
    data: &[u8],
    max: usize,
) -> io::Result<()> {
    if data.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let len = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    io.write_all(&len.to_be_bytes()).await?;
    io.write_all(data).await?;
    io.close().await
}

impl Codec for BytesCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io, MAX_PAYLOAD).await
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io, MAX_REPLY).await
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        req: Vec<u8>,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &req, MAX_PAYLOAD).await
    }

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        res: Vec<u8>,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &res, MAX_REPLY).await
    }
}
