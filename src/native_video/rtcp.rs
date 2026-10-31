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
    fn nack_encodes_the_bitmask() {
        let p = build_nack(1, 2, &[100, 101, 103]).unwrap();
        assert_eq!(&p[12..14], &100u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([p[14], p[15]]), 0b101);
    }
}
