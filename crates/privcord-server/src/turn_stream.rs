//! TURN over TCP and TLS, for phones on networks that block UDP.
//!
//! The client talks to this server over a byte stream (RFC 5766: STUN messages and ChannelData frames, the latter padded to
//! four bytes on a stream); what the server relays to the other side stays UDP, as usual. To the TURN server a stream looks
//! like one more socket: every frame that arrives is handed over together with the address it came from, and every reply is
//! written to the stream that address belongs to.

use async_trait::async_trait;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Semaphore};
use tokio_rustls::TlsAcceptor;
use webrtc_util::{Conn, Error as UtilError};

/// A frame bigger than this is not TURN.
const MAX_FRAME: usize = 8192;
/// No bytes for this long: the connection is dropped (clients refresh well inside it).
const IDLE: Duration = Duration::from_secs(15 * 60);
const MAX_STREAMS: usize = 8192;
const MAX_PER_IP: usize = 100;

pub struct StreamMux {
    inbound: tokio::sync::Mutex<mpsc::Receiver<(Vec<u8>, SocketAddr)>>,
    inbound_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    writers: Mutex<HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    local: SocketAddr,
    closed: AtomicBool,
}

impl std::fmt::Debug for StreamMux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamMux")
    }
}

impl StreamMux {
    pub fn new(local: SocketAddr) -> Arc<Self> {
        let (inbound_tx, rx) = mpsc::channel(2048);
        Arc::new(Self {
            inbound: tokio::sync::Mutex::new(rx),
            inbound_tx,
            writers: Mutex::new(HashMap::new()),
            per_ip: Mutex::new(HashMap::new()),
            local,
            closed: AtomicBool::new(false),
        })
    }

    fn writers(&self) -> std::sync::MutexGuard<'_, HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>> {
        self.writers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn admit(&self, ip: IpAddr) -> bool {
        let mut m = self.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        let n = m.entry(ip).or_insert(0);
        if *n >= MAX_PER_IP {
            return false;
        }
        *n += 1;
        true
    }

    fn release(&self, ip: IpAddr) {
        let mut m = self.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = m.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&ip);
            }
        }
    }

    /// How many stream clients are connected.
    pub fn clients(&self) -> usize {
        self.writers().len()
    }
}

#[async_trait]
impl Conn for StreamMux {
    async fn connect(&self, _addr: SocketAddr) -> webrtc_util::Result<()> {
        Err(UtilError::Other(
            "a TURN stream listener does not connect out".into(),
        ))
    }

    async fn recv(&self, buf: &mut [u8]) -> webrtc_util::Result<usize> {
        self.recv_from(buf).await.map(|(n, _)| n)
    }

    async fn recv_from(&self, buf: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        let mut rx = self.inbound.lock().await;
        let Some((data, from)) = rx.recv().await else {
            return Err(UtilError::ErrUseClosedNetworkConn);
        };
        let n = data.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..n), data.get(..n)) {
            dst.copy_from_slice(src);
        }
        Ok((n, from))
    }

    async fn send(&self, _buf: &[u8]) -> webrtc_util::Result<usize> {
        Err(UtilError::Other(
            "a TURN stream listener has no single peer".into(),
        ))
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> webrtc_util::Result<usize> {
        let tx = self.writers().get(&target).cloned();
        let Some(tx) = tx else {
            return Err(UtilError::Other(
                "that client is no longer connected".into(),
            ));
        };
        let mut frame = buf.to_vec();
        // ChannelData (first bits 01) is padded to a multiple of four on a stream; STUN messages already are.
        if buf.first().is_some_and(|b| b >> 6 == 1) {
            frame.resize(frame.len().div_ceil(4) * 4, 0);
        }
        // A client that cannot take what is sent is treated like a congested network: the frame is dropped.
        let _ = tx.try_send(frame);
        Ok(buf.len())
    }

    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        Ok(self.local)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }

    async fn close(&self) -> webrtc_util::Result<()> {
        self.closed.store(true, Ordering::Relaxed);
        self.writers().clear();
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// From the first four bytes of a frame: its length without padding, and with it. `None`: not TURN, hang up.
pub fn frame_lengths(head: [u8; 4]) -> Option<(usize, usize)> {
    let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
    let (total, padded) = match head[0] >> 6 {
        0 => (20 + len, 20 + len),
        1 => (4 + len, 4 + len.div_ceil(4) * 4),
        _ => return None, // neither STUN nor ChannelData
    };
    (padded <= MAX_FRAME).then_some((total, padded))
}

/// Reads STUN messages and ChannelData frames from a stream and passes each on, until it ends or turns out not to be TURN.
async fn read_frames<R: AsyncRead + Unpin>(mut r: R, peer: SocketAddr, mux: &StreamMux) {
    loop {
        let mut head = [0u8; 4];
        match tokio::time::timeout(IDLE, r.read_exact(&mut head)).await {
            Ok(Ok(_)) => {}
            _ => return,
        }
        let Some((total, padded)) = frame_lengths(head) else {
            return;
        };
        let mut msg = head.to_vec();
        msg.resize(padded, 0);
        let (_, rest) = msg.split_at_mut(4);
        if tokio::time::timeout(Duration::from_secs(30), r.read_exact(rest))
            .await
            .map_or(true, |x| x.is_err())
        {
            return;
        }
        msg.truncate(total);
        if mux.inbound_tx.send((msg, peer)).await.is_err() {
            return;
        }
    }
}

async fn serve_stream<S: AsyncRead + AsyncWrite + Send + 'static>(
    mux: Arc<StreamMux>,
    stream: S,
    peer: SocketAddr,
) {
    let (r, mut w) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);
    mux.writers().insert(peer, tx);
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if w.write_all(&frame).await.is_err() {
                break;
            }
        }
    });
    read_frames(r, peer, &mux).await;
    mux.writers().remove(&peer);
    writer.abort();
}

/// Accepts clients on `listener` (over TLS when `tls` is given) and feeds them to `mux` until `stop` fires.
pub fn spawn_listener(
    mux: Arc<StreamMux>,
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    mut stop: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let slots = Arc::new(Semaphore::new(MAX_STREAMS));
    tokio::spawn(async move {
        loop {
            let (tcp, peer) = tokio::select! {
                r = listener.accept() => match r {
                    Ok(x) => x,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                },
                _ = stop.changed() => return,
            };
            let _ = tcp.set_nodelay(true);
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            if !mux.admit(peer.ip()) {
                continue;
            }
            let (mux, tls) = (mux.clone(), tls.clone());
            tokio::spawn(async move {
                match tls {
                    Some(acceptor) => {
                        if let Ok(Ok(s)) =
                            tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp))
                                .await
                        {
                            serve_stream(mux.clone(), s, peer).await;
                        }
                    }
                    None => serve_stream(mux.clone(), tcp, peer).await,
                }
                mux.release(peer.ip());
                drop(permit);
            });
        }
    })
}
