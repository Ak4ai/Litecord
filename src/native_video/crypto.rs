//! Transport encryption for Discord voice/video (`*_rtpsize` modes).
//!
//! The 4 bytes appended to every packet are the nonce/counter.  The fixed RTP
//! header (plus the extension preamble) is authenticated as associated data,
//! while header-extension elements and the payload are encrypted.
//!
//! Discord has shipped slightly different nonce layouts over the years and
//! clients interoperate by trying the known ones, so the same strategy is used
//! here.  A failed AEAD tag simply means "try the next candidate".

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use super::rtp::RtpHeader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Aes256Gcm,
    XChaCha20Poly1305,
}

impl TransportMode {
    pub fn from_name(name: &str) -> Self {
        if name.to_ascii_lowercase().contains("xchacha") {
            Self::XChaCha20Poly1305
        } else {
            Self::Aes256Gcm
        }
    }
}

fn aes_nonces(suffix: &[u8; 4]) -> [[u8; 12]; 6] {
    let le = u32::from_le_bytes(*suffix);
    let be = u32::from_be_bytes(*suffix);
    let mut n1 = [0u8; 12];
    n1[0..4].copy_from_slice(suffix);
    let mut n2 = [0u8; 12];
    n2[8..12].copy_from_slice(suffix);
    let mut n3 = [0u8; 12];
    n3[0..4].copy_from_slice(&le.to_be_bytes());
    let mut n4 = [0u8; 12];
    n4[8..12].copy_from_slice(&le.to_be_bytes());
    let mut n5 = [0u8; 12];
    n5[0..4].copy_from_slice(&be.to_le_bytes());
    let mut n6 = [0u8; 12];
    n6[8..12].copy_from_slice(&be.to_le_bytes());
    [n1, n2, n3, n4, n5, n6]
}

fn xchacha_nonces(suffix: &[u8; 4]) -> [[u8; 24]; 4] {
    let le = u32::from_le_bytes(*suffix);
    let be = u32::from_be_bytes(*suffix);
    let mut n1 = [0u8; 24];
    n1[0..4].copy_from_slice(suffix);
    let mut n2 = [0u8; 24];
    n2[20..24].copy_from_slice(suffix);
    let mut n3 = [0u8; 24];
    n3[0..4].copy_from_slice(&le.to_be_bytes());
    let mut n4 = [0u8; 24];
    n4[0..4].copy_from_slice(&be.to_le_bytes());
    [n1, n2, n3, n4]
}

/// Removes RTP padding once the payload is in clear text (the count byte is the
/// last plaintext byte and is itself part of the padding).
fn strip_padding(plain: &mut Vec<u8>, header: &RtpHeader) {
    if !header.padding {
        return;
    }
    let Some(&count) = plain.last() else { return };
    let count = count as usize;
    if count > 0 && count <= plain.len() {
        plain.truncate(plain.len() - count);
    }
}

/// Protects an RTCP feedback packet with the connection transport key
/// (`*_rtpsize` modes).
///
/// The 8-byte RTCP header (version/type/length/sender SSRC) stays clear and is
/// authenticated as associated data; the report/FCI bytes are encrypted and the
/// 4-byte nonce counter is appended, exactly like RTP.  `counter` must be unique
/// per key on this connection.
pub fn protect_rtcp(key: &[u8], mode: TransportMode, packet: &[u8], counter: u32) -> Option<Vec<u8>> {
    if key.len() != 32 || packet.len() < 8 {
        return None;
    }
    let (aad, body) = packet.split_at(8);
    let suffix = counter.to_be_bytes();

    let ciphertext = match mode {
        TransportMode::Aes256Gcm => {
            let cipher = Aes256Gcm::new_from_slice(key).ok()?;
            let mut nonce = [0u8; 12];
            nonce[0..4].copy_from_slice(&suffix);
            cipher
                .encrypt(Nonce::from_slice(&nonce), Payload { msg: body, aad })
                .ok()?
        }
        TransportMode::XChaCha20Poly1305 => {
            let cipher = XChaCha20Poly1305::new_from_slice(key).ok()?;
            let mut nonce = [0u8; 24];
            nonce[20..24].copy_from_slice(&suffix);
            cipher
                .encrypt(XNonce::from_slice(&nonce), Payload { msg: body, aad })
                .ok()?
        }
    };

    let mut out = Vec::with_capacity(packet.len() + 20);
    out.extend_from_slice(aad);
    out.extend_from_slice(&ciphertext);
    out.extend_from_slice(&suffix);
    Some(out)
}

/// Tries to decrypt one RTP datagram and returns the H.264 payload.
///
/// `packet` still contains the 4-byte nonce suffix.  `header` is the parsed
/// cleartext header of the same packet.
pub fn decrypt_payload(
    key: &[u8],
    mode: TransportMode,
    packet: &[u8],
    header: &RtpHeader,
) -> Option<Vec<u8>> {
    if key.len() != 32 || packet.len() < header.payload_offset + 4 {
        return None;
    }
    let ciphertext_end = packet.len().saturating_sub(4);
    if ciphertext_end <= header.header_len {
        return None;
    }

    let suffix: [u8; 4] = packet[packet.len() - 4..].try_into().ok()?;

    // (associated-data length, bytes to drop from the start of the plaintext)
    // The first candidate is the common Discord layout (extension preamble is
    // clear, extension elements and payload are encrypted).
    let candidates: &[(usize, usize)] = if header.extension {
        &[
            (header.header_len + 4, header.extension_len - 4),
            (header.payload_offset, 0),
            (header.header_len, header.extension_len),
            (header.header_len, 0),
        ]
    } else {
        &[(header.header_len, 0)]
    };

    match mode {
        TransportMode::Aes256Gcm => {
            let cipher = Aes256Gcm::new_from_slice(key).ok()?;
            for &(aad_len, skip) in candidates {
                if aad_len >= ciphertext_end {
                    continue;
                }
                let aad = &packet[..aad_len];
                let ciphertext = &packet[aad_len..ciphertext_end];
                for nonce in aes_nonces(&suffix) {
                    if let Ok(mut plain) = cipher.decrypt(Nonce::from_slice(&nonce), Payload { msg: ciphertext, aad }) {
                        if plain.len() >= skip {
                            plain.drain(..skip);
                            strip_padding(&mut plain, header);
                            return Some(plain);
                        }
                    }
                }
            }
            None
        }
        TransportMode::XChaCha20Poly1305 => {
            let cipher = XChaCha20Poly1305::new_from_slice(key).ok()?;
            for &(aad_len, skip) in candidates {
                if aad_len >= ciphertext_end {
                    continue;
                }
                let aad = &packet[..aad_len];
                let ciphertext = &packet[aad_len..ciphertext_end];
                for nonce in xchacha_nonces(&suffix) {
                    if let Ok(mut plain) = cipher.decrypt(XNonce::from_slice(&nonce), Payload { msg: ciphertext, aad }) {
                        if plain.len() >= skip {
                            plain.drain(..skip);
                            strip_padding(&mut plain, header);
                            return Some(plain);
                        }
                    }
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypt(key: &[u8], packet_header: &[u8], payload: &[u8], suffix: [u8; 4]) -> Vec<u8> {
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&suffix);
        let mut out = packet_header.to_vec();
        let mut ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: payload, aad: packet_header })
            .unwrap();
        out.append(&mut ciphertext);
        out.extend_from_slice(&suffix);
        out
    }

    #[test]
    fn rtcp_keeps_its_header_clear_and_round_trips() {
        let key = [3u8; 32];
        let pli = super::super::rtcp::build_pli(11, 22);
        let wire = protect_rtcp(&key, TransportMode::Aes256Gcm, &pli, 0x8000_0001).unwrap();

        // RTCP header (sender SSRC included) stays readable on the wire.
        assert_eq!(&wire[..8], &pli[..8]);
        assert_eq!(&wire[wire.len() - 4..], &0x8000_0001u32.to_be_bytes());

        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&0x8000_0001u32.to_be_bytes());
        let clear = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload { msg: &wire[8..wire.len() - 4], aad: &wire[..8] },
            )
            .unwrap();
        assert_eq!(clear, &pli[8..]);
    }

    #[test]
    fn round_trips_a_rtpsize_packet() {
        let key = [7u8; 32];
        let rtp = vec![0x80, 103, 0, 1, 0, 0, 0, 0, 0, 0, 0, 9];
        let payload = [0x65, 0x88, 0x84, 0x00, 0x10];
        let packet = encrypt(&key, &rtp, &payload, [1, 2, 3, 4]);
        let h = super::super::rtp::parse(&packet).unwrap();
        assert_eq!(h.ssrc, 9);
        let clear = decrypt_payload(&key, TransportMode::Aes256Gcm, &packet, &h).unwrap();
        assert_eq!(clear, payload);
    }
}
