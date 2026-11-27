//! Finding this machine's public address, for servers behind a home router or a cloud's address translation.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const STUN_HOSTS: [&str; 3] = [
    "stun.l.google.com:19302",
    "stun.cloudflare.com:3478",
    "stun1.l.google.com:19302",
];

/// Asks public STUN servers what address our traffic comes from. Only a standard binding request is sent: no names, no ids.
pub async fn public_ip() -> Result<IpAddr, String> {
    let mut last = "no STUN server answered".to_owned();
    for host in STUN_HOSTS {
        match ask(host).await {
            Ok(ip) => return Ok(ip),
            Err(e) => last = format!("{host}: {e}"),
        }
    }
    Err(format!(
        "could not find the public address ({last}); set turn.public_ip by hand"
    ))
}

async fn ask(host: &str) -> Result<IpAddr, String> {
    let target: SocketAddr = tokio::net::lookup_host(host)
        .await
        .map_err(|e| e.to_string())?
        .find(SocketAddr::is_ipv4)
        .ok_or("no IPv4 address")?;
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut req = [0u8; 20];
    req[0] = 0x00;
    req[1] = 0x01; // binding request
    req[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]); // magic cookie
    getrandom::fill(&mut req[8..20]).map_err(|e| e.to_string())?;
    sock.send_to(&req, target)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(3), sock.recv(&mut buf))
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|e| e.to_string())?;
    parse_response(buf.get(..n).unwrap_or(&[]), req.get(8..20).unwrap_or(&[]))
}

/// The address in a STUN binding answer (XOR-MAPPED-ADDRESS, or the older MAPPED-ADDRESS), if it answers `txn`.
pub fn parse_response(msg: &[u8], txn: &[u8]) -> Result<IpAddr, String> {
    if msg.len() < 20 || msg.get(0..2) != Some(&[0x01, 0x01]) || msg.get(8..20) != Some(txn) {
        return Err("not an answer to our request".into());
    }
    let mut rest = msg.get(20..).unwrap_or(&[]);
    while let (Some(&[k0, k1, l0, l1]), Some(body)) = (
        rest.get(..4).and_then(|h| <&[u8; 4]>::try_from(h).ok()),
        rest.get(4..),
    ) {
        let kind = u16::from_be_bytes([k0, k1]);
        let len = usize::from(u16::from_be_bytes([l0, l1]));
        let val = body.get(..len).ok_or("truncated")?;
        if let (true, Some([0, 1, _, _, a, b, c, d])) = (
            kind == 0x0020 || kind == 0x0001,
            val.get(..8).and_then(|v| <&[u8; 8]>::try_from(v).ok()),
        ) {
            let mut ip = [*a, *b, *c, *d];
            if kind == 0x0020 {
                for (x, k) in ip.iter_mut().zip([0x21, 0x12, 0xa4, 0x42]) {
                    *x ^= k;
                }
            }
            return Ok(IpAddr::from(ip));
        }
        rest = body.get(len.div_ceil(4) * 4..).unwrap_or(&[]);
    }
    Err("no address in the answer".into())
}

/// Whether `ip` is one that can be reached from the internet (not private, loopback, or carrier-grade NAT).
pub fn is_public(ip: IpAddr) -> bool {
    !crate::turn_role::is_blocked(ip)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_xor_mapped_address() {
        let txn = [7u8; 12];
        let mut m = vec![0x01, 0x01, 0, 12, 0x21, 0x12, 0xa4, 0x42];
        m.extend_from_slice(&txn);
        // 203.0.113.9 xor cookie
        m.extend_from_slice(&[
            0x00,
            0x20,
            0,
            8,
            0,
            1,
            0x11,
            0x2b,
            203 ^ 0x21,
            0x12,
            113 ^ 0xa4,
            9 ^ 0x42,
        ]);
        assert_eq!(parse_response(&m, &txn), Ok("203.0.113.9".parse().unwrap()));
        assert!(parse_response(&m, &[8u8; 12]).is_err());
        assert!(parse_response(&m[..10], &txn).is_err());
    }
}
