//! H.264 RTP depacketizer (RFC 6184) for Discord video.
//!
//! Supports the packetization modes Discord uses: single NAL unit, STAP-A and
//! FU-A.  The output is an Annex B byte stream ready for `decode_video_frame`
//! and for DAVE (DAVE works on whole encoded frames, not on RTP payloads).

const START_CODE: [u8; 4] = [0, 0, 0, 1];

#[derive(Default)]
pub struct H264Depacketizer {
    frame: Vec<u8>,
    /// Partially received FU-A NAL unit.
    fu: Option<FuState>,
}

struct FuState {
    nal_header: u8,
    data: Vec<u8>,
}

impl H264Depacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.frame.is_empty() && self.fu.is_none()
    }

    /// Adds one RTP payload.  Returns the complete Annex B frame when the
    /// packet carried the marker bit.
    pub fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return if marker && !self.frame.is_empty() {
                Some(std::mem::take(&mut self.frame))
            } else {
                None
            };
        }

        let nal_type = payload[0] & 0x1F;
        match nal_type {
            // Single NAL unit packet.
            1..=23 => {
                self.frame.extend_from_slice(&START_CODE);
                self.frame.extend_from_slice(payload);
            }
            // STAP-A: one byte header, then [16-bit length][NAL]...
            24 => {
                let mut i = 1usize;
                while i + 2 <= payload.len() {
                    let len = u16::from_be_bytes([payload[i], payload[i + 1]]) as usize;
                    i += 2;
                    if len == 0 || i + len > payload.len() {
                        break;
                    }
                    self.frame.extend_from_slice(&START_CODE);
                    self.frame.extend_from_slice(&payload[i..i + len]);
                    i += len;
                }
            }
            // FU-A fragmentation.
            28 => {
                if payload.len() < 2 {
                    return None;
                }
                let fu_header = payload[1];
                let start = fu_header & 0x80 != 0;
                let end = fu_header & 0x40 != 0;
                let reconstructed = (payload[0] & 0xE0) | (fu_header & 0x1F);

                if start {
                    self.fu = Some(FuState { nal_header: reconstructed, data: payload[2..].to_vec() });
                } else if let Some(fu) = self.fu.as_mut() {
                    fu.data.extend_from_slice(&payload[2..]);
                }

                if end {
                    if let Some(fu) = self.fu.take() {
                        self.frame.extend_from_slice(&START_CODE);
                        self.frame.push(fu.nal_header);
                        self.frame.extend_from_slice(&fu.data);
                    }
                }
            }
            // Reserved / unspecified (0, 30, 31): ignore.
            _ => {}
        }

        if marker && !self.frame.is_empty() {
            self.fu = None;
            Some(std::mem::take(&mut self.frame))
        } else {
            None
        }
    }
}

/// Whether an Annex B frame contains an IDR slice (the pictures a decoder can
/// start from).
pub fn has_idr(frame: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 <= frame.len() {
        let start = if frame[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if frame[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        let header_index = i + start;
        if header_index < frame.len() && (frame[header_index] & 0x1F) == 5 {
            return true;
        }
        i = header_index + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAL: [u8; 4] = [0x65, 0x88, 0x84, 0x00];

    #[test]
    fn single_nal_frames_are_annex_b() {
        let mut d = H264Depacketizer::new();
        assert!(d.push(&NAL, false).is_none());
        let frame = d.push(&[0x41, 0x9A], true).unwrap();
        assert_eq!(&frame[..4], &START_CODE);
        assert_eq!(&frame[4..8], &NAL);
        assert_eq!(&frame[8..12], &START_CODE);
        assert!(has_idr(&frame));
    }

    #[test]
    fn fu_a_reassembles_the_nal() {
        let mut d = H264Depacketizer::new();
        // Original NAL header 0x65 -> FU indicator 0x7C, FU header S=1 type=5.
        assert!(d.push(&[0x7C, 0x85, 0x88, 0x84], false).is_none());
        assert!(d.push(&[0x7C, 0x05, 0x00, 0x11], false).is_none());
        let frame = d.push(&[0x7C, 0x45], true).unwrap();
        assert_eq!(frame, [&START_CODE[..], &[0x65, 0x88, 0x84, 0x00, 0x11]].concat());
    }

    #[test]
    fn stap_a_splits_each_nal() {
        let mut d = H264Depacketizer::new();
        let mut p = vec![24u8];
        p.extend_from_slice(&(4u16).to_be_bytes());
        p.extend_from_slice(&NAL);
        p.extend_from_slice(&(2u16).to_be_bytes());
        p.extend_from_slice(&[0x41, 0x9A]);
        let frame = d.push(&p, true).unwrap();
        assert_eq!(frame, [&START_CODE[..], &NAL[..], &START_CODE[..], &[0x41, 0x9A][..]].concat());
    }
}
