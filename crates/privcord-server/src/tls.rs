//! TLS for the web side (wss and the admin panel) and for TURN over TLS.
//!
//! The certificate comes from files you provide (and is read again when they change, so a renewed certificate is picked up
//! without a restart), or, for trying things out, a self-signed one that apps will not trust.

use crate::config::TlsConfig;
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::SystemTime;
use tokio_rustls::TlsAcceptor;

struct Resolver {
    current: RwLock<Arc<CertifiedKey>>,
    cert_file: PathBuf,
    key_file: PathBuf,
    stamp: Mutex<Option<(SystemTime, SystemTime)>>,
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Resolver")
    }
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(
            self.current
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn read_pair(cert_file: &Path, key_file: &Path) -> Result<Arc<CertifiedKey>, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_file)
        .map_err(|e| format!("could not read {}: {e}", cert_file.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bad certificate in {}: {e}", cert_file.display()))?;
    if chain.is_empty() {
        return Err(format!("{} holds no certificate", cert_file.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key_file)
        .map_err(|e| format!("could not read the private key {}: {e}", key_file.display()))?;
    let signing = any_supported_type(&key).map_err(|e| format!("unsupported private key: {e}"))?;
    Ok(Arc::new(CertifiedKey::new(chain, signing)))
}

/// What the web side and TURN-over-TLS use to accept connections.
pub struct Tls {
    resolver: Arc<Resolver>,
    /// For HTTPS: speaks HTTP/1.1.
    pub https: TlsAcceptor,
    /// For TURN over TLS.
    pub turn: TlsAcceptor,
    /// The certificate chain served now (DER), mostly so tests and `doctor` can show what is being offered.
    pub self_signed: bool,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tls")
    }
}

impl Tls {
    /// Loads the certificate named in `cfg`, or makes (and keeps in `data_dir/tls`) a self-signed one.
    pub fn load(cfg: &TlsConfig, data_dir: &Path, public_ip: &str) -> Result<Arc<Self>, String> {
        let (cert_file, key_file, self_signed) = if cfg.cert_file.is_empty() {
            if !cfg.self_signed {
                return Err("TLS needs tls.cert_file and tls.key_file (or tls.self_signed = true to try things out)".into());
            }
            let dir = data_dir.join("tls");
            let (c, k) = (dir.join("selfsigned.crt"), dir.join("selfsigned.key"));
            if !c.exists() || !k.exists() {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
                let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
                for n in [cfg.domain.as_str(), public_ip] {
                    if !n.is_empty() && !names.iter().any(|x| x == n) {
                        names.push(n.to_owned());
                    }
                }
                let made = rcgen::generate_simple_self_signed(names)
                    .map_err(|e| format!("could not make a certificate: {e}"))?;
                std::fs::write(&c, made.cert.pem())
                    .map_err(|e| format!("could not write {}: {e}", c.display()))?;
                std::fs::write(&k, made.key_pair.serialize_pem())
                    .map_err(|e| format!("could not write {}: {e}", k.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&k, std::fs::Permissions::from_mode(0o600));
                }
            }
            (c, k, true)
        } else {
            (
                PathBuf::from(&cfg.cert_file),
                PathBuf::from(&cfg.key_file),
                false,
            )
        };
        let current = read_pair(&cert_file, &key_file)?;
        let resolver = Arc::new(Resolver {
            current: RwLock::new(current),
            stamp: Mutex::new(modified(&cert_file).zip(modified(&key_file))),
            cert_file,
            key_file,
        });
        let build = |alpn: Vec<Vec<u8>>| -> Result<TlsAcceptor, String> {
            let mut c = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS setup: {e}"))?
            .with_no_client_auth()
            .with_cert_resolver(resolver.clone());
            c.alpn_protocols = alpn;
            Ok(TlsAcceptor::from(Arc::new(c)))
        };
        Ok(Arc::new(Self {
            https: build(vec![b"http/1.1".to_vec()])?,
            turn: build(vec![b"stun.turn".to_vec(), b"stun.nat-discovery".to_vec()])?,
            resolver,
            self_signed,
        }))
    }

    /// Reads the certificate files again if they changed. A bad new file leaves the old certificate in place.
    pub fn reload_if_changed(&self) -> Result<bool, String> {
        let now = modified(&self.resolver.cert_file).zip(modified(&self.resolver.key_file));
        let mut stamp = self
            .resolver
            .stamp
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if now == *stamp {
            return Ok(false);
        }
        let fresh = read_pair(&self.resolver.cert_file, &self.resolver.key_file)?;
        *self
            .resolver
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner) = fresh;
        *stamp = now;
        Ok(true)
    }

    /// The first certificate being served, DER (for a client that wants to trust it, as the tests do).
    pub fn leaf_der(&self) -> Vec<u8> {
        self.resolver
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .cert
            .first()
            .map(|c| c.as_ref().to_vec())
            .unwrap_or_default()
    }
}

/// A listener for `axum::serve` that completes the TLS handshake before handing a connection over. Handshakes run in their
/// own tasks, with a time limit and a cap on how many may be under way, so a slow or hostile client cannot hold up the rest.
pub struct TlsListener {
    rx: tokio::sync::mpsc::Receiver<(
        tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        std::net::SocketAddr,
    )>,
    addr: std::net::SocketAddr,
}

impl std::fmt::Debug for TlsListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TlsListener")
    }
}

impl TlsListener {
    pub fn new(listener: tokio::net::TcpListener, acceptor: TlsAcceptor) -> std::io::Result<Self> {
        let addr = listener.local_addr()?;
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let limit = Arc::new(tokio::sync::Semaphore::new(512));
        tokio::spawn(async move {
            loop {
                let Ok((tcp, peer)) = listener.accept().await else {
                    // Out of file handles or similar: wait a moment instead of spinning.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                };
                let _ = tcp.set_nodelay(true);
                let Ok(permit) = limit.clone().try_acquire_owned() else {
                    continue; // too many handshakes under way: drop this one
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let done = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        acceptor.accept(tcp),
                    )
                    .await;
                    drop(permit);
                    if let Ok(Ok(stream)) = done {
                        let _ = tx.send((stream, peer)).await;
                    }
                });
            }
        });
        Ok(Self { rx, addr })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(x) => x,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn client_for(der: Vec<u8>) -> tokio_rustls::TlsConnector {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(der)).unwrap();
        let c = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        tokio_rustls::TlsConnector::from(Arc::new(c))
    }

    async fn exchange(tls: &Tls, connector: &tokio_rustls::TlsConnector) -> String {
        let (a, b) = tokio::io::duplex(8192);
        let acceptor = tls.https.clone();
        let server = tokio::spawn(async move {
            let mut s = acceptor.accept(b).await.unwrap();
            s.write_all(b"hello").await.unwrap();
            s.shutdown().await.unwrap();
        });
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut c = connector.connect(name, a).await.unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).await.unwrap();
        server.await.unwrap();
        out
    }

    #[tokio::test]
    async fn a_self_signed_certificate_is_made_kept_and_served() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TlsConfig {
            self_signed: true,
            ..TlsConfig::default()
        };
        let tls = Tls::load(&cfg, dir.path(), "").unwrap();
        assert!(tls.self_signed);
        assert_eq!(exchange(&tls, &client_for(tls.leaf_der())).await, "hello");
        // the same certificate is used after a restart, so a client that pinned it keeps working
        let again = Tls::load(&cfg, dir.path(), "").unwrap();
        assert_eq!(again.leaf_der(), tls.leaf_der());
        // a client that does not trust it is refused
        let stranger = Tls::load(&cfg, tempfile::tempdir().unwrap().path(), "").unwrap();
        let (a, b) = tokio::io::duplex(8192);
        let acceptor = tls.https.clone();
        tokio::spawn(async move {
            let _ = acceptor.accept(b).await;
        });
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        assert!(client_for(stranger.leaf_der())
            .connect(name, a)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_renewed_certificate_is_picked_up_and_a_broken_one_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TlsConfig {
            self_signed: true,
            ..TlsConfig::default()
        };
        let tls = Tls::load(&cfg, dir.path(), "").unwrap();
        let old = tls.leaf_der();
        assert!(!tls.reload_if_changed().unwrap(), "nothing changed yet");
        // renew: write a new pair over the files
        let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let (c, k) = (
            dir.path().join("tls/selfsigned.crt"),
            dir.path().join("tls/selfsigned.key"),
        );
        std::thread::sleep(std::time::Duration::from_millis(1100)); // file times can be coarse
        std::fs::write(&c, made.cert.pem()).unwrap();
        std::fs::write(&k, made.key_pair.serialize_pem()).unwrap();
        assert!(tls.reload_if_changed().unwrap());
        assert_ne!(tls.leaf_der(), old);
        assert_eq!(exchange(&tls, &client_for(tls.leaf_der())).await, "hello");
        // a broken file: an error, and the working certificate stays
        let good = tls.leaf_der();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&c, "not a certificate").unwrap();
        assert!(tls.reload_if_changed().is_err());
        assert_eq!(tls.leaf_der(), good);
    }

    #[test]
    fn missing_files_are_explained() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TlsConfig {
            cert_file: "/nonexistent/c.pem".into(),
            key_file: "/nonexistent/k.pem".into(),
            ..TlsConfig::default()
        };
        let e = Tls::load(&cfg, dir.path(), "").unwrap_err();
        assert!(e.contains("could not read"), "{e}");
        assert!(Tls::load(&TlsConfig::default(), dir.path(), "")
            .unwrap_err()
            .contains("tls.cert_file"));
    }
}
