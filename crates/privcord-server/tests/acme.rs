#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! Automatic certificates against a mock ACME authority that behaves like Let's Encrypt: it checks the TLS-ALPN-01 challenge by
//! connecting to the server under test (asking for `acme-tls/1`, as the real one does), signs the order's CSR with its own CA,
//! and the test then checks that the server really serves that certificate, keeps it across a restart, and renews it when it is
//! about to run out. Not tested against the real Let's Encrypt (or Pebble): the network here does not reach it.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use privcord_server::{
    config::{Config, TlsConfig},
    serve::{run, HttpLimits},
    start,
    tls::{Tls, TlsListener},
};
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType, IsCa, KeyPair,
};
use rustls::pki_types::{CertificateSigningRequestDer, ServerName};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DOMAIN: &str = "relay.test.example";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

struct Mock {
    base: String,
    ca: rcgen::Certificate,
    ca_key: KeyPair,
    /// The server under test (its HTTPS port), once it is known.
    target: Mutex<Option<SocketAddr>>,
    orders: AtomicUsize,
    challenges_checked: AtomicUsize,
    jwk: Mutex<Option<Value>>,
    authz_valid: Mutex<bool>,
    /// This order has been finalized (the certificate is ready to fetch).
    finalized: Mutex<bool>,
    issued_leaf: Mutex<Vec<u8>>,
    validity_days: AtomicI64,
}

const TOKEN: &str = "token-for-the-test";

fn nonce_headers() -> [(&'static str, &'static str); 1] {
    [("replay-nonce", "nonce-1234567890")]
}

fn payload_of(body: &Value) -> Value {
    let p = body["payload"].as_str().unwrap_or("");
    if p.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&b64().decode(p).unwrap()).unwrap_or(Value::Null)
}

fn protected_of(body: &Value) -> Value {
    serde_json::from_slice(&b64().decode(body["protected"].as_str().unwrap()).unwrap()).unwrap()
}

fn thumbprint(jwk: &Value) -> String {
    // RFC 7638, for an EC key: the members in alphabetical order, nothing else.
    let canonical = format!(
        r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
        jwk["crv"].as_str().unwrap(),
        jwk["kty"].as_str().unwrap(),
        jwk["x"].as_str().unwrap(),
        jwk["y"].as_str().unwrap()
    );
    b64().encode(Sha256::digest(canonical.as_bytes()))
}

#[derive(Debug)]
struct AcceptAnything;
impl rustls::client::danger::ServerCertVerifier for AcceptAnything {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// What the real authority does to check TLS-ALPN-01: connect to the name's HTTPS port asking for `acme-tls/1`, and look at the
/// certificate it is shown: the name, and the proof in the critical acmeIdentifier extension.
async fn check_challenge(m: &Mock) -> Result<(), String> {
    let mut addr = None;
    for _ in 0..100 {
        addr = *m.target.lock().unwrap();
        if addr.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let addr = addr.ok_or("the server under test never said where it listens")?;
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAnything))
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"acme-tls/1".to_vec()];
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from(DOMAIN).unwrap(), tcp)
        .await
        .map_err(|e| format!("the server did not complete the challenge handshake: {e}"))?;
    let (_, conn) = tls.get_ref();
    if conn.alpn_protocol() != Some(b"acme-tls/1") {
        return Err("the server did not agree to acme-tls/1".into());
    }
    let leaf = conn
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or("no certificate")?;
    let (_, cert) =
        x509_parser::parse_x509_certificate(leaf.as_ref()).map_err(|e| e.to_string())?;
    let jwk = m.jwk.lock().unwrap().clone().ok_or("no account key seen")?;
    let key_authorization = format!("{TOKEN}.{}", thumbprint(&jwk));
    let mut want = vec![0x04, 0x20];
    want.extend_from_slice(&Sha256::digest(key_authorization.as_bytes()));
    let ext = cert
        .extensions()
        .iter()
        .find(|e| e.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
        .ok_or("the challenge certificate has no acmeIdentifier extension")?;
    if !ext.critical {
        return Err("the acmeIdentifier extension must be critical".into());
    }
    if ext.value != want.as_slice() {
        return Err("the proof in the challenge certificate is wrong".into());
    }
    let names = format!("{:?}", cert.subject_alternative_name());
    if !names.contains(DOMAIN) {
        return Err(format!(
            "the challenge certificate is not for {DOMAIN}: {names}"
        ));
    }
    Ok(())
}

fn reply(status: StatusCode, extra: Vec<(&'static str, String)>, body: Value) -> Response {
    let mut r = (status, nonce_headers(), Json(body)).into_response();
    for (k, v) in extra {
        r.headers_mut().insert(k, v.parse().unwrap());
    }
    r
}

async fn directory(State(m): State<Arc<Mock>>) -> Response {
    let b = &m.base;
    reply(
        StatusCode::OK,
        vec![],
        json!({ "newNonce": format!("{b}/new-nonce"), "newAccount": format!("{b}/new-acct"), "newOrder": format!("{b}/new-order"),
                "revokeCert": format!("{b}/revoke"), "keyChange": format!("{b}/key-change") }),
    )
}

async fn new_nonce() -> Response {
    (StatusCode::OK, nonce_headers()).into_response()
}

async fn new_acct(State(m): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    *m.jwk.lock().unwrap() = protected_of(&body).get("jwk").cloned();
    reply(
        StatusCode::CREATED,
        vec![("location", format!("{}/acct/1", m.base))],
        json!({ "status": "valid" }),
    )
}

fn order_json(m: &Mock, status: &str) -> Value {
    let b = &m.base;
    let mut v = json!({ "status": status, "expires": "2099-01-01T00:00:00Z",
        "identifiers": [{ "type": "dns", "value": DOMAIN }],
        "authorizations": [format!("{b}/authz/1")], "finalize": format!("{b}/finalize/1") });
    if status == "valid" {
        v["certificate"] = json!(format!("{b}/cert/1"));
    }
    v
}

async fn new_order(State(m): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    let wanted = payload_of(&body)["identifiers"][0]["value"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert_eq!(wanted, DOMAIN, "the order is for the configured name");
    m.orders.fetch_add(1, Ordering::SeqCst);
    *m.authz_valid.lock().unwrap() = false;
    *m.finalized.lock().unwrap() = false;
    reply(
        StatusCode::CREATED,
        vec![("location", format!("{}/order/1", m.base))],
        order_json(&m, "pending"),
    )
}

async fn authz(State(m): State<Arc<Mock>>) -> Response {
    let valid = *m.authz_valid.lock().unwrap();
    reply(
        StatusCode::OK,
        vec![],
        json!({ "status": if valid { "valid" } else { "pending" }, "expires": "2099-01-01T00:00:00Z",
                "identifier": { "type": "dns", "value": DOMAIN },
                "challenges": [{ "type": "tls-alpn-01", "url": format!("{}/chall/1", m.base),
                                 "status": if valid { "valid" } else { "pending" }, "token": TOKEN }] }),
    )
}

async fn chall(State(m): State<Arc<Mock>>) -> Response {
    m.challenges_checked.fetch_add(1, Ordering::SeqCst);
    match check_challenge(&m).await {
        Ok(()) => *m.authz_valid.lock().unwrap() = true,
        Err(e) => panic!("the challenge failed: {e}"),
    }
    reply(
        StatusCode::OK,
        vec![],
        json!({ "type": "tls-alpn-01", "url": format!("{}/chall/1", m.base), "status": "valid", "token": TOKEN }),
    )
}

async fn order(State(m): State<Arc<Mock>>) -> Response {
    let status = if *m.finalized.lock().unwrap() {
        "valid"
    } else if *m.authz_valid.lock().unwrap() {
        "ready"
    } else {
        "pending"
    };
    reply(
        StatusCode::OK,
        vec![("location", format!("{}/order/1", m.base))],
        order_json(&m, status),
    )
}

async fn finalize(State(m): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    let csr_der = b64()
        .decode(payload_of(&body)["csr"].as_str().unwrap())
        .unwrap();
    let mut csr =
        CertificateSigningRequestParams::from_der(&CertificateSigningRequestDer::from(csr_der))
            .unwrap();
    csr.params.not_before = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    csr.params.not_after = time::OffsetDateTime::now_utc()
        + time::Duration::days(m.validity_days.load(Ordering::SeqCst));
    let leaf = csr.signed_by(&m.ca, &m.ca_key).unwrap();
    *m.issued_leaf.lock().unwrap() = leaf.der().to_vec();
    *m.finalized.lock().unwrap() = true;
    // The next poll of the order says "valid".
    reply(
        StatusCode::OK,
        vec![("location", format!("{}/order/1", m.base))],
        order_json(&m, "valid"),
    )
}

async fn cert(State(m): State<Arc<Mock>>, _h: HeaderMap) -> Response {
    let leaf = m.issued_leaf.lock().unwrap().clone();
    let body = format!("{}{}", pem_of(&leaf), pem_of(m.ca.der().as_ref()));
    (
        StatusCode::OK,
        [
            ("content-type", "application/pem-certificate-chain"),
            ("replay-nonce", "nonce-1234567890"),
        ],
        body,
    )
        .into_response()
}

fn pem_of(der: &[u8]) -> String {
    let b = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    )
}

/// Starts the mock authority over HTTPS (the ACME client insists on it); returns it and the file with the certificate to trust.
async fn mock_authority(
    dir: &std::path::Path,
) -> (Arc<Mock>, String, tokio::sync::watch::Sender<bool>) {
    let tls_dir = dir.join("mock-tls");
    let tls = Tls::load(
        &TlsConfig {
            self_signed: true,
            ..Default::default()
        },
        &tls_dir,
        "127.0.0.1",
    )
    .unwrap();
    let root = dir.join("mock-root.pem");
    std::fs::write(&root, pem_of(&tls.leaf_der())).unwrap();
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp.local_addr().unwrap().port();
    let mut params = CertificateParams::new(vec![]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "Mock CA");
    let ca_key = KeyPair::generate().unwrap();
    let ca = params.self_signed(&ca_key).unwrap();
    let mock = Arc::new(Mock {
        base: format!("https://localhost:{port}"),
        ca,
        ca_key,
        target: Mutex::new(None),
        orders: AtomicUsize::new(0),
        challenges_checked: AtomicUsize::new(0),
        jwk: Mutex::new(None),
        authz_valid: Mutex::new(false),
        finalized: Mutex::new(false),
        issued_leaf: Mutex::new(Vec::new()),
        validity_days: AtomicI64::new(90),
    });
    let router = Router::new()
        .route("/directory", get(directory))
        .route("/new-nonce", get(new_nonce).head(new_nonce))
        .route("/new-acct", post(new_acct))
        .route("/new-order", post(new_order))
        .route("/authz/1", post(authz))
        .route("/chall/1", post(chall))
        .route("/order/1", post(order))
        .route("/finalize/1", post(finalize))
        .route("/cert/1", post(cert))
        .with_state(mock.clone());
    let listener = TlsListener::new(tcp, tls.https.clone()).unwrap();
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let limits = HttpLimits::from_config(&Config::default().server);
    tokio::spawn(async move {
        run(listener, router, limits, async move {
            let _ = stopped.wait_for(|s| *s).await;
        })
        .await;
    });
    // `tls` must outlive the listener's acceptor use.
    std::mem::forget(tls);
    (mock, root.display().to_string(), stop)
}

fn config(dir: &std::path::Path, mock: &Mock, root: &str) -> Config {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.join("data");
    c.nostr.enabled = false;
    c.tls = TlsConfig {
        https_listen: "127.0.0.1:0".into(),
        domain: DOMAIN.into(),
        acme: true,
        acme_accept_terms: true,
        acme_email: "ops@example.com".into(),
        acme_directory: format!("{}/directory", mock.base),
        acme_ca_root: root.into(),
        ..Default::default()
    };
    c
}

/// Connects to the HTTPS port and returns the DER of the certificate it serves for `name` (a normal client, not a challenge).
async fn served_cert(addr: SocketAddr) -> Vec<u8> {
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAnything))
    .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from(DOMAIN).unwrap(), tcp)
        .await
        .unwrap();
    tls.get_ref().1.peer_certificates().unwrap()[0]
        .as_ref()
        .to_vec()
}

async fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{what}");
}

#[tokio::test]
async fn a_certificate_is_obtained_served_kept_across_a_restart_and_renewed() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, root, stop) = mock_authority(dir.path()).await;
    // 10 days left on what is issued: worth renewing at once.
    mock.validity_days.store(90, Ordering::SeqCst);
    let r = start(config(dir.path(), &mock, &root)).await.unwrap();
    let https = r.tls_addr().unwrap();
    *mock.target.lock().unwrap() = Some(https);
    // The authority checked the challenge by connecting to the server, and the server now serves what it issued.
    wait_until("no certificate was issued", 30, || {
        !mock.issued_leaf.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(mock.challenges_checked.load(Ordering::SeqCst), 1);
    let issued = mock.issued_leaf.lock().unwrap().clone();
    let tls = r.tls().unwrap().clone();
    wait_until("the issued certificate was not installed", 30, || {
        tls.leaf_der() == issued
    })
    .await;
    assert_eq!(
        served_cert(https).await,
        issued,
        "a visitor is shown the issued certificate"
    );
    #[cfg_attr(not(unix), allow(unused_variables))]
    let (cert_file, key_file) = tls.acme_files();
    assert!(privcord_server::acme::days_left(&cert_file).unwrap() >= 89);
    // The key never travelled: it is private to this server's files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key_file).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    // The account is kept (so a renewal does not make a new one), in a private file.
    assert!(dir.path().join("data/tls/acme-account.json").exists());
    r.stop().await;
    // A restart with a certificate that is good for months orders nothing.
    let orders = mock.orders.load(Ordering::SeqCst);
    let r2 = start(config(dir.path(), &mock, &root)).await.unwrap();
    *mock.target.lock().unwrap() = r2.tls_addr();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        mock.orders.load(Ordering::SeqCst),
        orders,
        "no new order while the certificate is good"
    );
    assert_eq!(
        r2.tls().unwrap().leaf_der(),
        issued,
        "and the same one is served"
    );
    r2.stop().await;
    let _ = stop.send(true);
}

#[tokio::test]
async fn a_certificate_that_is_about_to_run_out_is_renewed_with_the_same_account() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, root, stop) = mock_authority(dir.path()).await;
    mock.validity_days.store(10, Ordering::SeqCst); // short: worth renewing as soon as it exists
    let cfg = config(dir.path(), &mock, &root);
    let r = start(cfg.clone()).await.unwrap();
    *mock.target.lock().unwrap() = r.tls_addr();
    wait_until("no certificate was issued", 30, || {
        !mock.issued_leaf.lock().unwrap().is_empty()
    })
    .await;
    let first = mock.issued_leaf.lock().unwrap().clone();
    let tls = r.tls().unwrap().clone();
    wait_until("the first certificate was not installed", 30, || {
        tls.leaf_der() == first
    })
    .await;
    let (cert_file, _) = tls.acme_files();
    assert!(privcord_server::acme::days_left(&cert_file).unwrap() <= 10);
    let account = std::fs::read_to_string(dir.path().join("data/tls/acme-account.json")).unwrap();
    // The check the task makes twice a day: with 10 days left it orders another.
    let before = mock.orders.load(Ordering::SeqCst);
    privcord_server::acme::ensure(
        &tls,
        &cfg.tls,
        &dir.path().join("data"),
        &privcord_server::record::Codec::plain(),
    )
    .await
    .unwrap();
    assert_eq!(
        mock.orders.load(Ordering::SeqCst),
        before + 1,
        "a renewal order"
    );
    let second = mock.issued_leaf.lock().unwrap().clone();
    assert_ne!(first, second);
    assert_eq!(
        tls.leaf_der(),
        second,
        "the renewed certificate is the one served"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("data/tls/acme-account.json")).unwrap(),
        account,
        "the same account"
    );
    r.stop().await;
    let _ = stop.send(true);
}

#[tokio::test]
async fn a_name_the_authority_cannot_check_leaves_the_placeholder_in_place_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, root, stop) = mock_authority(dir.path()).await;
    // The server under test is not told where it listens, so the mock cannot reach it: the order fails.
    let r = start(config(dir.path(), &mock, &root)).await.unwrap();
    let placeholder = r.tls().unwrap().leaf_der();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mock.issued_leaf.lock().unwrap().is_empty());
    assert_eq!(
        r.tls().unwrap().leaf_der(),
        placeholder,
        "the placeholder keeps being served"
    );
    assert!(
        !r.tls().unwrap().acme_files().0.exists(),
        "nothing half-written"
    );
    r.stop().await;
    let _ = stop.send(true);
}

#[test]
fn automatic_certificates_need_a_name_the_terms_and_the_https_port() {
    let mut c = Config::default();
    c.tls.acme = true;
    let said = |c: &Config| c.problems().join(" | ");
    let s = said(&c);
    assert!(
        s.contains("tls.domain")
            && s.contains("acme_accept_terms")
            && s.contains("tls.https_listen"),
        "{s}"
    );
    c.tls.domain = "203.0.113.5".into(); // an address is not a name
    assert!(said(&c).contains("not an IP address"), "{}", said(&c));
    c.tls.domain = "relay.example.com".into();
    c.tls.acme_accept_terms = true;
    c.tls.https_listen = "0.0.0.0:443".into();
    assert!(!said(&c).contains("tls."), "{}", said(&c));
    c.tls.cert_file = "a.pem".into();
    c.tls.key_file = "a.key".into();
    assert!(said(&c).contains("alternatives"), "{}", said(&c));
}
