//! Minimal RTP/RTCP header handling for Discord native video.
//!
//! Discord multiplexes RTP and RTCP on the voice UDP socket.  Only the clear
//! part of the header is needed here: the transport layer encrypts the payload
//! (and the header-extension elements), not the fixed header.

/// Parsed view of an RTP packet.  `header_len` does not include the extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub csrc_count: u8,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    /// Fixed header + CSRC list.
    pub header_len: usize,
    /// Extension preamble (4) + extension words, zero when `extension` is false.
    pub extension_len: usize,
    /// Where the (encrypted) payload starts when the extension is present.
    pub payload_offset: usize,
    /// Padding bytes declared by the last payload byte.
    pub padding_len: usize,
}

impl RtpHeader {
    /// Length of the header used as AEAD associated data for `rtpsize` modes:
    /// fixed header + CSRCs + the 4-byte extension preamble.
    pub fn aad_len(&self) -> usize {
        if self.extension {
            self.header_len + 4
        } else {
            self.header_len
        }
    }
}

/// Returns true when the first bytes describe RTCP rather than RTP.
///
/// RTCP packet types live in 200..=207 (SR, RR, SDES, BYE, APP, RTPFB, PSFB,
/// XR).  Discord only uses a few of them, but rejecting the whole range keeps
/// control packets out of the video depacketizer.
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 8 && (packet[0] >> 6) == 2 && (200..=207).contains(&packet[1])
}

/// Parses the clear RTP header.  Returns `None` for truncated or non-v2 packets.
pub fn parse(packet: &[u8]) -> Option<RtpHeader> {
    if packet.len() < 12 || (packet[0] >> 6) != 2 {
        return None;
    }

    let padding = packet[0] & 0x20 != 0;
    let extension = packet[0] & 0x10 != 0;
    let csrc_count = packet[0] & 0x0F;
    let header_len = 12 + 4 * csrc_count as usize;
    if packet.len() < header_len {
        return None;
    }

    let mut extension_len = 0usize;
    if extension {
        if packet.len() < header_len + 4 {
            return None;
        }
        let words = u16::from_be_bytes([packet[header_len + 2], packet[header_len + 3]]) as usize;
        extension_len = 4 + words * 4;
        if packet.len() < header_len + extension_len {
            return None;
        }
    }

    // The last byte of an encrypted datagram is part of the 4-byte nonce suffix,
    // not the RTP padding count, so it cannot be read before decryption.  Video is
    // never padded in practice; `decrypt_payload` strips padding after decrypting
    // when the flag is set.
    let padding_len = 0usize;

    Some(RtpHeader {
        version: 2,
        padding,
        extension,
        csrc_count,
        marker: packet[1] & 0x80 != 0,
        payload_type: packet[1] & 0x7F,
        sequence: u16::from_be_bytes([packet[2], packet[3]]),
        timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        header_len,
        extension_len,
        payload_offset: header_len + extension_len,
        padding_len,
    })
}

/// End of the RTP payload, excluding the AEAD nonce suffix handled elsewhere.
pub fn payload_end(packet_len: usize, header: &RtpHeader) -> usize {
    packet_len
        .saturating_sub(header.padding_len)
        .max(header.payload_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp(payload: &[u8], marker: bool, seq: u16, ts: u32, ssrc: u32) -> Vec<u8> {
        let mut p = vec![0x80, if marker { 0x80 | 103 } else { 103 }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&ssrc.to_be_bytes());
        p.extend_from_slice(payload);
        p
    }

    #[test]
    fn parses_basic_rtp() {
        let p = rtp(&[1, 2, 3], true, 77, 90000, 1234);
        let h = parse(&p).unwrap();
        assert_eq!(h.payload_type, 103);
        assert_eq!(h.sequence, 77);
        assert_eq!(h.timestamp, 90000);
        assert_eq!(h.ssrc, 1234);
        assert!(h.marker);
        assert_eq!(h.payload_offset, 12);
        assert_eq!(payload_end(p.len(), &h), p.len());
    }

    #[test]
    fn skips_csrc_and_extension() {
        let mut p = vec![0x80 | 0x10 | 2, 103, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2];
        p.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2]); // two CSRCs
        p.extend_from_slice(&[0xBE, 0xDE, 0, 1]); // one extension word
        p.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        p.extend_from_slice(&[9, 9, 9]);
        let h = parse(&p).unwrap();
        assert_eq!(h.csrc_count, 2);
        assert_eq!(h.header_len, 20);
        assert_eq!(h.extension_len, 8);
        assert_eq!(h.payload_offset, 28);
        assert_eq!(h.aad_len(), 24);
        assert_eq!(&p[h.payload_offset..], &[9, 9, 9]);
    }

    #[test]
    fn detects_rtcp() {
        let sr = [0x80, 200, 0, 6, 0, 0, 0, 1];
        assert!(is_rtcp(&sr));
        let r = rtp(&[1], false, 0, 0, 1);
        assert!(!is_rtcp(&r));
    }
}
