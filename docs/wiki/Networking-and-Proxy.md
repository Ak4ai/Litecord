# 🌐 Networking & Proxy Architecture

Litecord includes native, enterprise-grade proxy support and network resilience capabilities.

---

## 1. Supported Proxy Protocols

- **HTTP / HTTPS Proxy**: Full support for standard forward proxies and HTTPS `CONNECT` tunneling.
- **SOCKS5 Proxy**: Compatible with Tor, Shadowsocks, V2Ray, and local proxies (`socks5://127.0.0.1:10808`).
- **OS System Proxy**: Automatically detects system proxy configurations from Windows Internet Settings.

---

## 2. Dynamic Hot-Reloading

Modifying proxy configurations in Settings immediately reconstructs the underlying `reqwest::Client` in memory without restarting the application:
```rust
let builder = apply_proxy_to_builder(reqwest::Client::builder(), &settings);
let new_client = builder.build().unwrap();
```

---

## 3. Proactive Connection Diagnostics

- **Real-Time Gateway Ping**: Periodically verifies round-trip time (RTT) to `gateway.discord.gg`.
- **Intelligent Error Differentiation**: Differentiates between authentication errors (`401 Unauthorized`) and proxy connection drops (`10061 Connection Refused`).
- **Emergency Shortcut**: If a configured proxy goes offline, an interactive banner appears on the login screen with a one-click shortcut to open Settings and disable the proxy.
