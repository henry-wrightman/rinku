//! Custom CBOR codec with configurable max message sizes for libp2p request-response.
//! This allows checkpoint vote requests with large transaction payloads to succeed.
//! Uses unsigned-varint length prefixes for wire-compatibility with libp2p.

use futures::prelude::*;
use libp2p::request_response::Codec;
use libp2p::StreamProtocol;
use serde::{de::DeserializeOwned, Serialize};
use std::{io, marker::PhantomData, pin::Pin};

/// Custom CBOR codec with configurable max request/response sizes.
///
/// Unlike the built-in `libp2p::request_response::cbor` codec which has
/// a hardcoded ~1MB limit, this codec allows configuring larger limits
/// for protocols that need to transfer larger payloads (like checkpoint
/// vote requests with embedded transactions).
#[derive(Clone)]
pub struct CborCodec<Req, Resp> {
    /// Maximum size of a serialized request in bytes
    max_request_size: usize,
    /// Maximum size of a serialized response in bytes  
    max_response_size: usize,
    _phantom: PhantomData<(Req, Resp)>,
}

impl<Req, Resp> CborCodec<Req, Resp> {
    /// Create a new CBOR codec with specified max sizes.
    ///
    /// # Arguments
    /// * `max_request_size` - Maximum size for request messages in bytes
    /// * `max_response_size` - Maximum size for response messages in bytes
    pub fn new(max_request_size: usize, max_response_size: usize) -> Self {
        Self {
            max_request_size,
            max_response_size,
            _phantom: PhantomData,
        }
    }
}

impl<Req, Resp> Default for CborCodec<Req, Resp> {
    fn default() -> Self {
        // Default to 16MB limits (much larger than built-in ~1MB)
        Self::new(16 * 1024 * 1024, 16 * 1024 * 1024)
    }
}

/// Read a varint-prefixed message from an async reader
async fn read_length_prefixed<T: AsyncRead + Unpin>(
    io: &mut T,
    max_size: usize,
) -> io::Result<Vec<u8>> {
    // Read varint length prefix byte by byte
    let mut len: usize = 0;
    let mut shift: u32 = 0;

    loop {
        let mut byte_buf = [0u8; 1];
        io.read_exact(&mut byte_buf).await?;
        let byte = byte_buf[0];

        // Add lower 7 bits to length
        len |= ((byte & 0x7f) as usize) << shift;
        shift += 7;

        // If high bit is 0, we're done
        if byte & 0x80 == 0 {
            break;
        }

        // Prevent overflow (varint too long)
        if shift >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Varint too long",
            ));
        }
    }

    if len > max_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Message size {} exceeds max {}", len, max_size),
        ));
    }

    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Write a varint-prefixed message to an async writer
async fn write_length_prefixed<T: AsyncWrite + Unpin>(
    io: &mut T,
    data: &[u8],
    max_size: usize,
) -> io::Result<()> {
    if data.len() > max_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Message size {} exceeds max {}", data.len(), max_size),
        ));
    }

    // Encode length as varint
    let mut len = data.len();
    let mut varint_buf = [0u8; 10]; // Max 10 bytes for 64-bit varint
    let mut i = 0;

    loop {
        let mut byte = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80; // Set continuation bit
        }
        varint_buf[i] = byte;
        i += 1;
        if len == 0 {
            break;
        }
    }

    // Write varint prefix
    io.write_all(&varint_buf[..i]).await?;
    // Write data
    io.write_all(data).await?;
    io.flush().await?;

    Ok(())
}

impl<Req, Resp> Codec for CborCodec<Req, Resp>
where
    Req: Send + Serialize + DeserializeOwned + 'static,
    Resp: Send + Serialize + DeserializeOwned + 'static,
{
    type Protocol = StreamProtocol;
    type Request = Req;
    type Response = Resp;

    fn read_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> Pin<Box<dyn Future<Output = io::Result<Self::Request>> + Send + 'async_trait>>
    where
        T: AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        let max_size = self.max_request_size;
        Box::pin(async move {
            let buf = read_length_prefixed(io, max_size).await?;
            serde_cbor::from_slice(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("CBOR decode error: {}", e),
                )
            })
        })
    }

    fn read_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send + 'async_trait>>
    where
        T: AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        let max_size = self.max_response_size;
        Box::pin(async move {
            let buf = read_length_prefixed(io, max_size).await?;
            serde_cbor::from_slice(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("CBOR decode error: {}", e),
                )
            })
        })
    }

    fn write_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        req: Self::Request,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        T: AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        let max_size = self.max_request_size;
        Box::pin(async move {
            let data = serde_cbor::to_vec(&req).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("CBOR encode error: {}", e),
                )
            })?;
            write_length_prefixed(io, &data, max_size).await
        })
    }

    fn write_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        resp: Self::Response,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        T: AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        let max_size = self.max_response_size;
        Box::pin(async move {
            let data = serde_cbor::to_vec(&resp).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("CBOR encode error: {}", e),
                )
            })?;
            write_length_prefixed(io, &data, max_size).await
        })
    }
}
