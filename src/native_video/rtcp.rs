//! RTCP control packets used by the native video receiver: picture-loss
//! indication (ask for a keyframe) and generic NACK (ask for retransmission).

fn word_len(packet_bytes: usize) -> u16 {
    ((packet_bytes / 4).saturating_sub(1)) as u16
}

/// Picture Loss Indication (PSFB, FMT=1).
pub fn build_pli(sender_ssrc: u32, media_ssrc: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.push(0x81); // version 2, FMT 1
    out.push(206); // PSFB
    out.extend_from_slice(&word_len(12).to_be_bytes());
    out.extend_from_slice(&sender_ssrc.to_be_bytes());
    out.extend_from_slice(&media_ssrc.to_be_bytes());
    out
}

/// Whether a (possibly compound) RTCP packet asks the sender for a keyframe.
///
/// Discord's SFU sends `PSFB` (206) with `FMT=1` (PLI) when a viewer starts
/// receiving, or `FMT=4` (FIR) after a decoder reset.  On the parent voice
/// connection these arrive interleaved with audio RTP, so the camera sender
/// can only react if the voice receive loop inspects them.
pub fn wants_keyframe(packet: &[u8]) -> bool {
    let mut offset = 0usize;
    while offset + 4 <= packet.len() {
        let version = packet[offset] >> 6;
        if version != 2 {
            return false;
        }
        let count = packet[offset] & 0x1F;
        let payload_type = packet[offset + 1];
        let len = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        let size = (len + 1) * 4;
        if size < 4 || offset + size > packet.len() {
            return false;
        }
        // PSFB (206): FMT 1 is PLI and FMT 4 is FIR.
        if payload_type == 206 && (count == 1 || count == 4) {
            return true;
        }
        offset += size;
    }
    false
}

/// Generic NACK (RTPFB, FMT=1) for up to 17 consecutive missing packets.
pub fn build_nack(sender_ssrc: u32, media_ssrc: u32, missing: &[u16]) -> Option<Vec<u8>> {
    let first = *missing.first()?;
    let mut mask = 0u16;
    for seq in missing.iter().skip(1).take(16) {
        let delta = seq.wrapping_sub(first);
        if delta > 0 && delta <= 16 {
            mask |= 1 << (delta - 1);
        }
    }
    let mut out = Vec::with_capacity(16);
    out.push(0x81);
    out.push(205); // RTPFB
    out.extend_from_slice(&word_len(16).to_be_bytes());
    out.extend_from_slice(&sender_ssrc.to_be_bytes());
    out.extend_from_slice(&media_ssrc.to_be_bytes());
    out.extend_from_slice(&first.to_be_bytes());
    out.extend_from_slice(&mask.to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pli_has_the_expected_shape() {
        let p = build_pli(1, 2);
        assert_eq!(p, vec![0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
    }

    #[test]
    fn pli_and_fir_are_keyframe_requests() {
        let pli = build_pli(9, 10);
        assert!(wants_keyframe(&pli));

        // PSFB (206) with FMT 4 (FIR) and the mandatory FCI word.
        let fir = vec![0x84, 206, 0, 3, 0, 0, 0, 9, 0, 0, 0, 10, 0, 0, 0, 1];
        assert!(wants_keyframe(&fir));

        // A Sender Report is not.
        let mut sr = vec![0x80, 200, 0, 6, 0, 0, 0, 9];
        sr.extend_from_slice(&[0u8; 20]);
        assert!(!wants_keyframe(&sr));
    }

    #[test]
    fn nack_encodes_the_bitmask() {
        let p = build_nack(1, 2, &[100, 101, 103]).unwrap();
        assert_eq!(&p[12..14], &100u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([p[14], p[15]]), 0b101);
    }
}
