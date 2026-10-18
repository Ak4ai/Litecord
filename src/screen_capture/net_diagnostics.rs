//! Network diagnostics for screen sharing between different networks: what kind of NAT
//! this machine is behind, whether the signaling server answers, and which peers were
//! seen on which path. The result is a short verdict for the UI and a plain-text report
//! the user can paste into an issue.

use std::net::{IpAddr, SocketAddr};

/// What the NAT test says about direct (peer-to-peer) connections over the internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    // The discriminants are the codes the settings UI shows (net_diag_verdict).
    /// Same public address and port towards every server (or no NAT): hole punching works.
    Direct = 0,
    /// A different public port per destination (symmetric NAT): direct connections across
    /// the internet will usually fail; the local network still works.
    Symmetric = 1,
    /// A different public IP per destination (carrier-grade NAT pools, multiple uplinks).
    VaryingIp = 2,
    /// No STUN server answered: UDP to the internet looks blocked on this network.
    UdpBlocked = 3,
}

/// Classifies the NAT from the public addresses STUN servers reported for one local socket.
pub fn classify(mapped: &[Option<SocketAddr>]) -> Verdict {
    let answers: Vec<SocketAddr> = mapped.iter().flatten().copied().collect();
    let Some(first) = answers.first() else { return Verdict::UdpBlocked };
    if answers.iter().any(|a| a.ip() != first.ip()) {
        Verdict::VaryingIp
    } else if answers.iter().any(|a| a.port() != first.port()) {
        Verdict::Symmetric
    } else {
        Verdict::Direct
    }
}

/// Outcome of a diagnostics run.
pub struct Diagnosis {
    pub verdict: Verdict,
    /// Round trip to the signaling server, if it answered.
    pub signaling_ms: Option<u128>,
    /// Plain-text report for an issue.
    pub report: String,
}

/// Three providers on three different IPs: a symmetric NAT shows a different public port
/// for at least one of them.
const STUN_SERVERS: [&str; 3] = ["stun.l.google.com:19302", "stun.cloudflare.com:3478", "global.stun.twilio.com:3478"];

/// Runs the checks (a few seconds of blocking network I/O; call it off the UI thread).
pub fn run() -> Diagnosis {
    use std::fmt::Write;
    let mut r = String::new();
    let _ = writeln!(r, "Litecord network diagnostics");
    let _ = writeln!(r, "version: {} ({}) | os: {} {}", env!("CARGO_PKG_VERSION"),
        option_env!("LITECORD_GIT_HASH").unwrap_or("?"), std::env::consts::OS, std::env::consts::ARCH);

    let lan: Vec<String> = super::get_local_lan_addresses(0)
        .iter()
        .filter(|a| !a.ip().is_loopback())
        .map(|a| a.ip().to_string())
        .collect();
    let _ = writeln!(r, "local addresses: {}", if lan.is_empty() { "none found".to_string() } else { lan.join(", ") });

    let net = crate::utils::network_settings::get_network_settings();
    let _ = writeln!(r, "proxy: {}{}", net.proxy_mode,
        if net.proxy_mode != "off" { format!(" (media through proxy: {})", net.route_media) } else { String::new() });

    // NAT: one socket, several servers.
    let mut mapped = Vec::new();
    match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(sock) => {
            let local_port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
            let _ = writeln!(r, "STUN from one UDP socket (local port {local_port}):");
            for (i, server) in STUN_SERVERS.iter().enumerate() {
                let (result, line) = stun_query(&sock, server, i as u8);
                mapped.push(result);
                let _ = writeln!(r, "  {server}: {line}");
            }
            if let Some(Some(a)) = mapped.iter().find(|m| m.is_some()) {
                let _ = writeln!(r, "  local port {}preserved by the router", if a.port() == local_port { "" } else { "not " });
            }
        }
        Err(e) => {
            let _ = writeln!(r, "STUN: could not open a UDP socket: {e}");
        }
    }
    let verdict = classify(&mapped);
    let _ = writeln!(r, "verdict: {}", match verdict {
        Verdict::Direct => "direct connections should work (same public address towards every server)",
        Verdict::Symmetric => "symmetric NAT: a new public port per destination, direct connections across the internet will usually fail",
        Verdict::VaryingIp => "a different public IP per destination (carrier-grade NAT or several uplinks): direct connections may fail",
        Verdict::UdpBlocked => "no STUN server answered: UDP to the internet looks blocked",
    });

    let rx_port = super::get_my_rx_port();
    let p2p_public = super::CACHED_STUN_ADDR.lock().ok().and_then(|g| g.map(|(a, _)| a));
    let _ = writeln!(r, "stream socket: local port {rx_port}, public {}",
        p2p_public.map_or("not resolved yet (join a call)".to_string(), mask_public));

    // Signaling server reachability.
    let url = super::signaling::signaling_base_url().replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
    let start = std::time::Instant::now();
    let signaling_ms = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()
        .and_then(|c| c.get(&url).send().ok())
        .map(|resp| {
            let ms = start.elapsed().as_millis();
            let _ = writeln!(r, "signaling server: answered HTTP {} in {ms} ms", resp.status().as_u16());
            ms
        });
    if signaling_ms.is_none() {
        let _ = writeln!(r, "signaling server: no answer within 5 s ({url})");
    }

    // Call and peers.
    let in_call = crate::gateway::is_connected_to_voice();
    let _ = writeln!(r, "in a voice call: {}", if in_call { "yes" } else { "no" });
    if let Ok(guard) = super::LAST_SEEN_PEER_ADDR.lock() {
        if let Some(map) = guard.as_ref() {
            for (uid, addr) in map {
                let path = match addr.ip() {
                    IpAddr::V4(v4) if v4.is_private() || v4.is_link_local() => "local network",
                    _ => "internet",
                };
                let _ = writeln!(r, "  peer {uid}: last seen at {} ({path})", mask_public(*addr));
            }
        }
    }

    Diagnosis { verdict, signaling_ms, report: r }
}

/// The report is meant for public issues: hide the last two parts of public IPv4 addresses
/// (ports, which tell the NAT behaviour, stay). Private addresses are shown as they are.
fn mask_public(addr: SocketAddr) -> String {
    match addr.ip() {
        IpAddr::V4(v4) if !(v4.is_private() || v4.is_link_local() || v4.is_loopback()) => {
            let [a, b, _, _] = v4.octets();
            format!("{a}.{b}.*.*:{}", addr.port())
        }
        _ => addr.to_string(),
    }
}

/// One STUN binding request; returns the public address and a line for the report.
fn stun_query(sock: &std::net::UdpSocket, server: &str, tag: u8) -> (Option<SocketAddr>, String) {
    use std::net::ToSocketAddrs;
    let Some(dest) = server.to_socket_addrs().ok().and_then(|mut it| it.find(|a| a.is_ipv4())) else {
        return (None, "could not resolve".to_string());
    };
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&[0x00, 0x01]); // Binding Request, no attributes
    req[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
    req[8..20].copy_from_slice(b"LitecordDia\0");
    req[19] = tag;
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(400)));
    let start = std::time::Instant::now();
    let mut buf = [0u8; 512];
    for _ in 0..3 {
        if sock.send_to(&req, dest).is_err() {
            return (None, format!("({}) send failed", dest.ip()));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            let Ok((len, _)) = sock.recv_from(&mut buf) else { break };
            if len >= 20 && buf[8..20] == req[8..20] {
                if let Some(mapped) = parse_mapped_address(&buf[..len]) {
                    return (Some(mapped), format!("({}) -> {} in {} ms", dest.ip(), mask_public(mapped), start.elapsed().as_millis()));
                }
            }
        }
    }
    (None, format!("({}) no answer", dest.ip()))
}

/// XOR-MAPPED-ADDRESS (or MAPPED-ADDRESS) from a STUN Binding Success Response, IPv4 only.
pub(crate) fn parse_mapped_address(msg: &[u8]) -> Option<SocketAddr> {
    if msg.len() < 20 || msg[0..2] != [0x01, 0x01] {
        return None;
    }
    let mut i = 20;
    while i + 4 <= msg.len() {
        let kind = u16::from_be_bytes([msg[i], msg[i + 1]]);
        let len = u16::from_be_bytes([msg[i + 2], msg[i + 3]]) as usize;
        let value = msg.get(i + 4..i + 4 + len)?;
        if (kind == 0x0020 || kind == 0x0001) && len >= 8 && value[1] == 0x01 {
            let xor = kind == 0x0020;
            let port = u16::from_be_bytes([value[2], value[3]]) ^ if xor { 0x2112 } else { 0 };
            let cookie = [0x21u8, 0x12, 0xa4, 0x42];
            let ip: [u8; 4] = std::array::from_fn(|k| value[4 + k] ^ if xor { cookie[k] } else { 0 });
            return Some(SocketAddr::from((ip, port)));
        }
        i += 4 + ((len + 3) & !3);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Option<SocketAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn same_mapping_everywhere_is_direct() {
        assert_eq!(classify(&[addr("45.1.2.3:5000"), addr("45.1.2.3:5000"), None]), Verdict::Direct);
    }

    #[test]
    fn port_changing_per_server_is_symmetric() {
        assert_eq!(classify(&[addr("45.1.2.3:5000"), addr("45.1.2.3:5001")]), Verdict::Symmetric);
    }

    #[test]
    fn ip_changing_per_server_is_varying_ip() {
        assert_eq!(classify(&[addr("45.1.2.3:5000"), addr("45.9.9.9:5000")]), Verdict::VaryingIp);
    }

    #[test]
    fn no_answer_is_udp_blocked() {
        assert_eq!(classify(&[None, None, None]), Verdict::UdpBlocked);
        assert_eq!(classify(&[]), Verdict::UdpBlocked);
    }

    #[test]
    fn report_hides_public_addresses_but_not_ports_or_private_ones() {
        assert_eq!(mask_public("45.178.251.10:36194".parse().unwrap()), "45.178.*.*:36194");
        assert_eq!(mask_public("192.168.1.31:50005".parse().unwrap()), "192.168.1.31:50005");
    }

    #[test]
    fn parses_xor_mapped_address() {
        // Binding Success Response with XOR-MAPPED-ADDRESS 45.1.2.3:5000.
        let mut msg = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        msg.extend_from_slice(&[0u8; 12]);
        msg.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
        msg.extend_from_slice(&(5000u16 ^ 0x2112).to_be_bytes());
        msg.extend_from_slice(&[45 ^ 0x21, 1 ^ 0x12, 2 ^ 0xa4, 3 ^ 0x42]);
        assert_eq!(parse_mapped_address(&msg), addr("45.1.2.3:5000"));
    }

    #[test]
    fn a_single_answer_cannot_show_symmetry_and_counts_as_direct() {
        assert_eq!(classify(&[None, addr("45.1.2.3:5000")]), Verdict::Direct);
    }
}
