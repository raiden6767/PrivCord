//! A TURN client's connection over a byte stream (TCP or TLS): what a phone on a network that blocks UDP does.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use webrtc_util::{Conn, Error as UtilError};

pub struct StreamConn<S> {
    r: tokio::sync::Mutex<ReadHalf<S>>,
    w: tokio::sync::Mutex<WriteHalf<S>>,
    local: SocketAddr,
    remote: SocketAddr,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> StreamConn<S> {
    pub fn new(stream: S, local: SocketAddr, remote: SocketAddr) -> Self {
        let (r, w) = tokio::io::split(stream);
        Self {
            r: tokio::sync::Mutex::new(r),
            w: tokio::sync::Mutex::new(w),
            local,
            remote,
        }
    }

    async fn write_frame(&self, buf: &[u8]) -> webrtc_util::Result<usize> {
        let mut frame = buf.to_vec();
        if buf.first().is_some_and(|b| b >> 6 == 1) {
            frame.resize(frame.len().div_ceil(4) * 4, 0);
        }
        self.w
            .lock()
            .await
            .write_all(&frame)
            .await
            .map_err(|e| UtilError::Other(e.to_string()))?;
        Ok(buf.len())
    }
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static> Conn for StreamConn<S> {
    async fn connect(&self, _addr: SocketAddr) -> webrtc_util::Result<()> {
        Ok(())
    }
    async fn recv(&self, buf: &mut [u8]) -> webrtc_util::Result<usize> {
        let mut r = self.r.lock().await;
        let mut head = [0u8; 4];
        r.read_exact(&mut head)
            .await
            .map_err(|e| UtilError::Other(e.to_string()))?;
        let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
        let (total, padded) = match head[0] >> 6 {
            0 => (20 + len, 20 + len),
            1 => (4 + len, 4 + len.div_ceil(4) * 4),
            _ => return Err(UtilError::Other("not TURN".into())),
        };
        let mut msg = head.to_vec();
        msg.resize(padded, 0);
        let (_, rest) = msg.split_at_mut(4);
        r.read_exact(rest)
            .await
            .map_err(|e| UtilError::Other(e.to_string()))?;
        msg.truncate(total);
        buf[..total].copy_from_slice(&msg);
        Ok(total)
    }
    async fn recv_from(&self, buf: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        let n = self.recv(buf).await?;
        Ok((n, self.remote))
    }
    async fn send(&self, buf: &[u8]) -> webrtc_util::Result<usize> {
        self.write_frame(buf).await
    }
    async fn send_to(&self, buf: &[u8], _target: SocketAddr) -> webrtc_util::Result<usize> {
        self.write_frame(buf).await
    }
    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        Ok(self.local)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
    async fn close(&self) -> webrtc_util::Result<()> {
        let _ = self.w.lock().await.shutdown().await;
        Ok(())
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}
