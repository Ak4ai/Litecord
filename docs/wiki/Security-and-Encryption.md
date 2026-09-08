# 🛡️ Security & Encryption

Litecord is designed around local-first privacy, zero telemetry, and industry-standard cryptographic primitives.

---

## 1. Multi-Account Credential Vault

- **Windows**: Protected using the Windows Data Protection API (**DPAPI** - `CryptProtectData`). The master encryption key is derived from the user's OS login credentials and secured by the Windows LSA (Local Security Authority).
- **Linux**: Protected using **AES-256-GCM** with a hardware-anchored key stored in `~/.config/litecord/session.vault`.
- User tokens are never stored in plain text, never exported in logs, and immediately wiped from RAM on logout.

---

## 2. Voice & Video Cryptography

1. **Discord DAVE E2EE (Voice Calls)**:
   - Implements Messaging Layer Security (MLS) key negotiation.
   - All voice frames are encrypted at the client boundary using AES-GCM.
2. **X25519 ECDH + AES-256-GCM (P2P Video)**:
   - Direct screen-share sessions perform an ephemeral Curve25519 Diffie-Hellman key exchange over encrypted signaling.
   - AES-NI accelerated encryption delivers sub-0.05ms frame processing.

---

## 3. Zero-Knowledge Principles

- **No Remote Telemetry**: Litecord does not report usage data, analytics, or IP telemetry to third-party tracking servers.
- **Direct Discord Gateway**: All communication travels directly to official Discord endpoints (`discord.com`, `gateway.discord.gg`).
