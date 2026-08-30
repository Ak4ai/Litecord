# Security Policy & Privacy Architecture

At **Litecord**, security, privacy, and account safety are first-class engineering priorities. Litecord is designed to protect users against token exfiltration, network sniffing, and unauthorized access.

---

## 🛡️ Supported Versions

Only the latest release and the current `main` branch receive official security patches and updates.

| Version | Supported          |
| ------- | ------------------ |
| `v0.3.x` (Latest) | :white_check_mark: |
| `< v0.3.0`        | :x:                |

---

## 🔒 Core Security & Privacy Guarantees

1. **Local Cryptographic Vaults**:
   - **Linux**: Credentials are stored in `~/.config/litecord/session.vault` with strict Unix permissions (`0700` directory, `0600` file) and encrypted using **AES-256-GCM** derived from machine-specific hardware identifiers (`/etc/machine-id`) and user UID.
   - **Windows**: Stored in `%APPDATA%/Litecord/session.vault` encrypted via **Windows DPAPI** (`CryptProtectData`), bound to the local OS account.
2. **Zero-Token Logging & Atomic Purging**:
   - Tokens and authentication secrets are never output to logs, terminal stdout/stderr, or crash dumps.
   - Tokens are atomically erased from disk and memory when the user logs out.
3. **Direct Discord Connection (No Proxies)**:
   - Litecord connects directly to official Discord API and Gateway endpoints (`gateway.discord.gg`, `discord.com`).
   - No intermediary servers, third-party proxies, analytics, telemetry, or remote tracking scripts exist.
4. **End-to-End Encrypted Voice & Video (E2EE)**:
   - Voice and video streams utilize hardware-accelerated **AES-256-GCM** with per-room session key derivation and support for Discord's **DAVE / MLS (RFC 9420)** end-to-end voice encryption.
5. **Path Traversal & Shell Injection Defense**:
   - All attachment filenames and URI handlers are strictly sanitized to prevent directory traversal and arbitrary code execution.
   - External links are opened using native OS APIs (`xdg-open` / `ShellExecuteW`) with explicit `http://` / `https://` protocol whitelisting.

---

## 🚨 Reporting a Vulnerability

If you discover a security vulnerability in Litecord, please **do not report it publicly in GitHub Issues**.

Instead, please disclose responsibly:
1. Open a **[Private Security Advisory](https://github.com/Ak4ai/Litecord/security/advisories/new)** on GitHub.
2. Provide detailed steps to reproduce the vulnerability, including platform, version, and proof-of-concept if applicable.

We will investigate all legitimate reports promptly and coordinate a fix before public disclosure.
