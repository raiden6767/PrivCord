//! Asking the home router to forward the server's ports (UPnP Internet Gateway Device), so a server on a home connection can
//! be reached without anyone logging in to the router. Off unless `server.port_mapping = true`.
//!
//! Careful by design: only the ports this server listens on are mapped (never the admin panel's), every mapping has a lease
//! of an hour that is renewed while the server runs, so a crash leaves no hole behind for more than that, and the mappings are
//! removed when the server stops. A router that does not answer, or that sits behind its provider's shared address, is
//! reported in words (health report and `doctor`), not treated as an error.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

/// What the router is asked to hold each mapping for, and how often it is renewed (well inside the lease).
pub const LEASE_SECS: u32 = 3600;
const RENEW_EVERY: Duration = Duration::from_secs(1800);
const SSDP: &str = "239.255.255.250:1900";
const MAX_MAPPINGS: usize = 300;
const DESC: &str = "PrivCord server";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    fn name(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// Where the mapping stands, for the health report and the panel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Off,
    Searching,
    Mapped {
        external_ip: IpAddr,
        ports: usize,
    },
    /// The router answered but this address is not reachable from the internet (a shared provider address).
    Shared {
        external_ip: IpAddr,
    },
    Failed(String),
}

pub type Status = Arc<Mutex<State>>;

#[derive(Debug, Clone)]
pub struct Gateway {
    /// `host:port` of the router's UPnP web service.
    host: String,
    control_path: String,
    service: String,
    /// This machine's address as the router sees it (the mapping's target).
    local_ip: Ipv4Addr,
}

fn between<'a>(text: &'a str, open: &str, close: &str, from: usize) -> Option<(&'a str, usize)> {
    let a = text.get(from..)?.find(open)? + from + open.len();
    let b = text.get(a..)?.find(close)? + a;
    Some((text.get(a..b)?.trim(), b + close.len()))
}

/// The header `name:` of an HTTP-style message, case-insensitively.
fn header<'a>(msg: &'a str, name: &str) -> Option<&'a str> {
    msg.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// `http://host:port/path` into (`host:port`, `/path`).
fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
    Some((host.to_owned(), path))
}

async fn http(host: &str, request: String) -> Result<(u16, String), String> {
    let work = async {
        let mut s = TcpStream::connect(host).await.map_err(|e| e.to_string())?;
        s.write_all(request.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        // A router's answer is small; never read more than a few KB.
        s.take(64 * 1024)
            .read_to_end(&mut raw)
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        Ok::<_, String>((status, body.to_owned()))
    };
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .map_err(|_| "the router did not answer in time".to_owned())?
}

/// The router's IGD service, found by asking `target` (the multicast address in real use). `None` if nothing answers.
pub async fn discover_at(target: &str, wait: Duration) -> Result<Gateway, String> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n"
    );
    sock.send_to(search.as_bytes(), target)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = [0u8; 2048];
    let (n, from) = tokio::time::timeout(wait, sock.recv_from(&mut buf))
        .await
        .map_err(|_| "no router answered (UPnP may be switched off on it)".to_owned())?
        .map_err(|e| e.to_string())?;
    let reply = String::from_utf8_lossy(buf.get(..n).unwrap_or_default()).into_owned();
    let location = header(&reply, "location").ok_or("the router's answer names no description")?;
    let (host, path) =
        split_url(location).ok_or("the router's description address is not usable")?;
    // The description must come from the machine that answered: not whatever address a reply names.
    let answered_by = from.ip().to_string();
    if !host.starts_with(&answered_by) && host.split(':').next() != Some(answered_by.as_str()) {
        return Err("the router's description is on another machine; ignored".into());
    }
    let (status, xml) = http(
        &host,
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
    )
    .await?;
    if status != 200 {
        return Err(format!("the router's description answered {status}"));
    }
    // The first WAN connection service (IP or PPP) and its control address.
    let mut at = 0;
    let (service, control) = loop {
        let Some((kind, next)) = between(&xml, "<serviceType>", "</serviceType>", at) else {
            return Err("the router offers no port forwarding service".into());
        };
        at = next;
        if kind.contains(":WANIPConnection:") || kind.contains(":WANPPPConnection:") {
            let (c, _) =
                between(&xml, "<controlURL>", "</controlURL>", at).ok_or("no control address")?;
            break (kind.to_owned(), c.to_owned());
        }
    };
    let control_path = match split_url(&control) {
        Some((h, p)) if h == host => p,
        Some(_) => {
            return Err("the router's control address is on another machine; ignored".into())
        }
        None if control.starts_with('/') => control,
        None => format!("/{control}"),
    };
    let probe = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let gw: SocketAddr = host
        .parse()
        .map_err(|_| "the router's address is not usable".to_owned())?;
    probe.connect(gw).await.map_err(|e| e.to_string())?;
    let IpAddr::V4(local_ip) = probe.local_addr().map_err(|e| e.to_string())?.ip() else {
        return Err("this machine has no IPv4 address on the router's network".into());
    };
    Ok(Gateway {
        host,
        control_path,
        service,
        local_ip,
    })
}

pub async fn discover() -> Result<Gateway, String> {
    discover_at(SSDP, Duration::from_secs(4)).await
}

impl Gateway {
    async fn soap(&self, action: &str, args: &str) -> Result<String, String> {
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{svc}\">{args}</u:{action}></s:Body></s:Envelope>",
            svc = self.service
        );
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nSOAPAction: \"{}#{action}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.control_path,
            self.host,
            self.service,
            body.len()
        );
        let (status, text) = http(&self.host, req).await?;
        if status == 200 {
            Ok(text)
        } else {
            let why = between(&text, "<errorDescription>", "</errorDescription>", 0)
                .map_or("refused", |(d, _)| d);
            Err(format!("the router answered {status}: {why}"))
        }
    }

    /// The address the router has on the internet side.
    pub async fn external_ip(&self) -> Result<IpAddr, String> {
        let text = self.soap("GetExternalIPAddress", "").await?;
        let (ip, _) = between(
            &text,
            "<NewExternalIPAddress>",
            "</NewExternalIPAddress>",
            0,
        )
        .ok_or("the router gave no address")?;
        ip.parse()
            .map_err(|_| "the router's external address is not an address".to_owned())
    }

    pub async fn add(&self, proto: Proto, port: u16) -> Result<(), String> {
        self.soap(
            "AddPortMapping",
            &format!(
                "<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>{}</NewProtocol><NewInternalPort>{port}</NewInternalPort><NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>{DESC}</NewPortMappingDescription><NewLeaseDuration>{LEASE_SECS}</NewLeaseDuration>",
                proto.name(),
                self.local_ip
            ),
        )
        .await
        .map(|_| ())
    }

    pub async fn remove(&self, proto: Proto, port: u16) -> Result<(), String> {
        self.soap(
            "DeletePortMapping",
            &format!("<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>{}</NewProtocol>", proto.name()),
        )
        .await
        .map(|_| ())
    }
}

/// An address that is not the open internet (private, shared provider space): a mapping on such a router reaches nobody.
fn shared(ip: IpAddr) -> bool {
    !crate::nat::is_public(ip)
}

/// Keeps `ports` mapped until `stop`: finds the router, maps, renews every half hour, removes at the end.
pub async fn run(
    ports: Vec<(Proto, u16)>,
    status: Status,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    run_at(SSDP, ports, status, &mut stop, RENEW_EVERY).await;
}

pub async fn run_at(
    target: &str,
    ports: Vec<(Proto, u16)>,
    status: Status,
    stop: &mut tokio::sync::watch::Receiver<bool>,
    renew: Duration,
) {
    let set = |s: State| {
        *status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = s
    };
    let ports: Vec<(Proto, u16)> = ports.into_iter().take(MAX_MAPPINGS).collect();
    set(State::Searching);
    let gw = match discover_at(target, Duration::from_secs(4)).await {
        Ok(g) => g,
        Err(e) => return set(State::Failed(e)),
    };
    let mut mapped = Vec::new();
    loop {
        let external = gw.external_ip().await;
        mapped.clear();
        let mut first_error = None;
        for &(proto, port) in &ports {
            match gw.add(proto, port).await {
                Ok(()) => mapped.push((proto, port)),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match (external, mapped.is_empty()) {
            (_, true) => set(State::Failed(
                first_error.unwrap_or_else(|| "nothing to map".into()),
            )),
            (Ok(ip), false) if shared(ip) => set(State::Shared { external_ip: ip }),
            (Ok(external_ip), false) => set(State::Mapped {
                external_ip,
                ports: mapped.len(),
            }),
            (Err(e), false) => set(State::Failed(e)),
        }
        tokio::select! {
            () = tokio::time::sleep(renew) => {}
            _ = stop.wait_for(|s| *s) => break,
        }
    }
    // Leave nothing open behind us.
    for (proto, port) in mapped {
        let _ = gw.remove(proto, port).await;
    }
    set(State::Off);
}

/// The state in words, for the health report and `doctor`.
pub fn describe(s: &State) -> (bool, String) {
    match s {
        State::Off => (true, "Automatic port forwarding is off.".into()),
        State::Searching => (true, "Looking for the router to forward ports.".into()),
        State::Mapped { external_ip, ports } => (true, format!("The router forwards {ports} port(s) to this machine; the internet address is {external_ip}.")),
        State::Shared { external_ip } => (false, format!("The router's address ({external_ip}) is shared with other customers of your provider, so forwarding cannot make this server reachable from the internet. Use a small rented server, or a tunnel to one.")),
        State::Failed(why) => (false, format!("Automatic port forwarding did not work: {why}.")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A pretend router: answers the UDP search, serves a description and takes SOAP calls, recording them.
    struct Fake {
        udp: String,
        calls: Arc<Mutex<Vec<String>>>,
        _adds: Arc<AtomicUsize>,
    }

    async fn fake_router(external: &'static str) -> Fake {
        let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_addr = web.local_addr().unwrap();
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let adds = Arc::new(AtomicUsize::new(0));
        let (c, a) = (calls.clone(), adds.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = web.accept().await else {
                    return;
                };
                let (c, a) = (c.clone(), a.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let reply = if req.starts_with("GET /desc.xml") {
                        "<root><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/ctl</controlURL></service></serviceList></device></root>".to_owned()
                    } else if req.contains("#GetExternalIPAddress") {
                        format!("<NewExternalIPAddress>{external}</NewExternalIPAddress>")
                    } else {
                        if req.contains("#AddPortMapping") {
                            a.fetch_add(1, Ordering::SeqCst);
                        }
                        c.lock().unwrap().push(
                            req.lines()
                                .find(|l| l.starts_with("SOAPAction"))
                                .unwrap_or("")
                                .to_owned()
                                + &req[req.find("<NewExternalPort>").unwrap_or(0)
                                    ..req
                                        .find("</NewProtocol>")
                                        .unwrap_or(0)
                                        .max(req.find("<NewExternalPort>").unwrap_or(0))],
                        );
                        "<ok/>".to_owned()
                    };
                    let _ = s
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                                reply.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_addr = udp.local_addr().unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 1024];
            while let Ok((_, from)) = udp.recv_from(&mut b).await {
                let r = format!(
                    "HTTP/1.1 200 OK\r\nLOCATION: http://{web_addr}/desc.xml\r\nST: x\r\n\r\n"
                );
                let _ = udp.send_to(r.as_bytes(), from).await;
            }
        });
        Fake {
            udp: udp_addr.to_string(),
            calls,
            _adds: adds,
        }
    }

    #[tokio::test]
    async fn the_server_finds_the_router_maps_its_ports_renews_and_removes_them_at_the_end() {
        let fake = fake_router("203.0.113.5").await;
        let status: Status = Arc::default();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let st = status.clone();
        let target = fake.udp.clone();
        let task = tokio::spawn(async move {
            run_at(
                &target,
                vec![(Proto::Tcp, 8787), (Proto::Udp, 3478)],
                st,
                &mut stop_rx,
                Duration::from_millis(150),
            )
            .await;
        });
        let t = std::time::Instant::now();
        loop {
            let adds = fake
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.contains("#AddPortMapping"))
                .count();
            if adds >= 4 {
                break; // mapped once, then renewed
            }
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "not renewed: {:?}",
                fake.calls.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let now = status.lock().unwrap().clone();
        assert_eq!(
            now,
            State::Mapped {
                external_ip: "203.0.113.5".parse().unwrap(),
                ports: 2
            }
        );
        stop_tx.send(true).unwrap();
        task.await.unwrap();
        let calls = fake.calls.lock().unwrap().clone();
        let removed = calls
            .iter()
            .filter(|c| c.contains("#DeletePortMapping"))
            .count();
        assert_eq!(removed, 2, "every mapping is removed at the end: {calls:?}");
        assert_eq!(*status.lock().unwrap(), State::Off);
    }

    #[tokio::test]
    async fn a_router_with_a_shared_provider_address_is_reported_not_trusted() {
        let fake = fake_router("100.64.3.9").await;
        let status: Status = Arc::default();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let (st, target) = (status.clone(), fake.udp.clone());
        let task = tokio::spawn(async move {
            run_at(
                &target,
                vec![(Proto::Tcp, 8787)],
                st,
                &mut stop_rx,
                Duration::from_secs(60),
            )
            .await;
        });
        let t = std::time::Instant::now();
        while !matches!(*status.lock().unwrap(), State::Shared { .. }) {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "state: {:?}",
                status.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!describe(&status.lock().unwrap()).0);
        stop_tx.send(true).unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn nothing_answering_is_a_plain_failure() {
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let r = discover_at(
            &silent.local_addr().unwrap().to_string(),
            Duration::from_millis(200),
        )
        .await;
        assert!(r.unwrap_err().contains("no router answered"));
    }

    #[test]
    fn xml_pieces_and_urls_are_cut_out_safely() {
        assert_eq!(
            between("<a> x </a>", "<a>", "</a>", 0).map(|p| p.0),
            Some("x")
        );
        assert!(between("<a>x", "<a>", "</a>", 0).is_none());
        assert_eq!(
            split_url("http://10.0.0.1:5000/x/y"),
            Some(("10.0.0.1:5000".into(), "/x/y".into()))
        );
        assert_eq!(
            split_url("http://10.0.0.1"),
            Some(("10.0.0.1".into(), "/".into()))
        );
        assert!(split_url("ftp://x").is_none());
        assert_eq!(
            header("A: 1\r\nLocation: http://x\r\n", "location"),
            Some("http://x")
        );
    }
}
