use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

fn query_stun_server(socket: &UdpSocket, server: &str) -> Option<SocketAddr> {
    let addrs: Vec<SocketAddr> = server.to_socket_addrs().ok()?.collect();
    let stun_addr = addrs.iter().find(|a| a.is_ipv4())?;

    // STUN Binding Request (RFC 5389 / RFC 3489)
    let stun_req: [u8; 20] = [
        0x00, 0x01, // Binding Request
        0x00, 0x00, // Length
        0x21, 0x12, 0xa4, 0x42, // Magic Cookie
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, // Transaction ID
    ];

    let _ = socket.send_to(&stun_req, stun_addr);

    let mut buf = [0u8; 1024];
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        if let Ok((len, src)) = socket.recv_from(&mut buf) {
            if src.ip() == stun_addr.ip() && len >= 20 && buf[0] == 0x01 && buf[1] == 0x01 {
                let mut i = 20;
                while i + 4 <= len {
                    let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
                    let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
                    if i + 4 + attr_len > len { break; }
                    
                    // XOR-MAPPED-ADDRESS (0x0020) or MAPPED-ADDRESS (0x0001)
                    if attr_type == 0x0020 && attr_len >= 8 && buf[i + 5] == 0x01 {
                        let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]) ^ 0x2112;
                        let ip = std::net::Ipv4Addr::new(
                            buf[i + 8] ^ 0x21,
                            buf[i + 9] ^ 0x12,
                            buf[i + 10] ^ 0xa4,
                            buf[i + 11] ^ 0x42,
                        );
                        return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                    } else if attr_type == 0x0001 && attr_len >= 8 && buf[i + 5] == 0x01 {
                        let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]);
                        let ip = std::net::Ipv4Addr::new(buf[i + 8], buf[i + 9], buf[i + 10], buf[i + 11]);
                        return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                    }
                    i += 4 + ((attr_len + 3) & !3);
                }
            }
        }
    }
    None
}

fn main() {
    println!("==================================================================");
    println!("🌐 TEST_STUN_WAN: Teste Independente de Resolução Pública (WAN)");
    println!("==================================================================");

    let socket = UdpSocket::bind("0.0.0.0:50005")
        .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
        .expect("Falha ao abrir socket UDP");
    socket.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    println!("🎧 Socket local vinculado na porta: {}", socket.local_addr().unwrap().port());

    let stun_servers = [
        "stun.l.google.com:19302",
        "stun1.l.google.com:19302",
        "stun2.l.google.com:19302",
        "stun3.l.google.com:19302",
        "stun4.l.google.com:19302",
        "stun.cloudflare.com:3478",
        "stun.twilio.com:3478",
    ];

    println!("\n🔍 Consultando servidores STUN públicos...");
    let mut resolved_public_addr = None;

    for server in stun_servers {
        print!("   -> Consultando {} ... ", server);
        match query_stun_server(&socket, server) {
            Some(mapped_addr) => {
                println!("✅ MAPEADO: {}", mapped_addr);
                if resolved_public_addr.is_none() {
                    resolved_public_addr = Some(mapped_addr);
                }
            }
            None => {
                println!("❌ TIMEOUT");
            }
        }
    }

    match resolved_public_addr {
        Some(addr) => {
            println!("\n🎉 IP PÚBLICO WAN RESOLVIDO COM SUCESSO: {}", addr);
        }
        None => {
            println!("\n❌ ERRO: Não foi possível obter o IP público via nenhum servidor STUN.");
        }
    }
}
