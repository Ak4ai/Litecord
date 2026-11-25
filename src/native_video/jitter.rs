//! Per-SSRC reorder buffer for RTP video.
//!
//! Discord's SFU can reorder packets, and UDP loses some.  The buffer emits
//! payloads in sequence order and reports gaps so the receiver can send NACKs.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How far ahead of the expected sequence a packet may be buffered.
const MAX_AHEAD: i16 = 512;
/// Report at most this many missing packets per call (a single NACK FCI pair).
const MAX_REPORTED_GAPS: usize = 16;

/// One ordered RTP payload and its marker bit.
pub type OrderedPayload = (u16, Vec<u8>, bool);

#[derive(Default)]
pub struct JitterBuffer {
    expected: Option<u16>,
    packets: BTreeMap<u16, (Vec<u8>, bool)>,
    /// When the oldest still-missing packet was first noticed.
    gap_since: Option<Instant>,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the packets that can now be emitted in order, plus the sequence
    /// numbers that look lost.
    pub fn push(&mut self, seq: u16, payload: Vec<u8>, marker: bool) -> (Vec<OrderedPayload>, Vec<u16>) {
        let Some(expected) = self.expected else {
            self.expected = Some(seq);
            self.packets.insert(seq, (payload, marker));
            let mut out = Vec::new();
            self.drain_ready(&mut out);
            return (out, Vec::new());
        };

        let diff = seq.wrapping_sub(expected) as i16;
        if diff < 0 {
            // Late duplicate: already emitted, or too old to matter.
            return (Vec::new(), Vec::new());
        }
        if diff == 0 {
            self.packets.insert(seq, (payload, marker));
            let mut out = Vec::new();
            self.drain_ready(&mut out);
            return (out, Vec::new());
        }

        let mut missing = Vec::new();
        if diff > MAX_AHEAD {
            // The sender jumped (encoder restart, long outage): resync instead of
            // buffering forever.
            missing.extend(self.packets.keys().copied());
            self.packets.clear();
            self.expected = Some(seq);
            self.packets.insert(seq, (payload, marker));
            let mut out = Vec::new();
            self.drain_ready(&mut out);
            return (out, missing);
        }

        for offset in 0..diff.min(MAX_REPORTED_GAPS as i16) {
            let lost = expected.wrapping_add(offset as u16);
            if !self.packets.contains_key(&lost) {
                missing.push(lost);
            }
        }

        self.packets.insert(seq, (payload, marker));
        // Keep the map bounded even if a huge burst is lost.
        while self.packets.len() > MAX_AHEAD as usize {
            if let Some(first) = self.packets.keys().next().copied() {
                self.packets.remove(&first);
                self.expected = Some(first.wrapping_add(1));
            }
        }
        let mut out = Vec::new();
        self.drain_ready(&mut out);
        (out, missing)
    }

    /// Emits buffered packets past a gap that never filled (lost and not
    /// retransmitted).  Without this the video would freeze forever after the
    /// first lost packet.
    pub fn flush_stale(&mut self, max_wait: Duration, now: Instant) -> Vec<OrderedPayload> {
        let Some(since) = self.gap_since else {
            return Vec::new();
        };
        if now.duration_since(since) < max_wait {
            return Vec::new();
        }
        let Some(first) = self.packets.keys().next().copied() else {
            self.gap_since = None;
            return Vec::new();
        };
        // Skip whatever is missing and continue from the oldest packet we have.
        self.expected = Some(first);
        let mut out = Vec::new();
        self.drain_ready(&mut out);
        out
    }

    fn drain_ready(&mut self, out: &mut Vec<OrderedPayload>) {
        while let Some(expected) = self.expected {
            let Some((payload, marker)) = self.packets.remove(&expected) else {
                break;
            };
            out.push((expected, payload, marker));
            self.expected = Some(expected.wrapping_add(1));
        }
        if self.packets.is_empty() {
            self.gap_since = None;
        } else if self.gap_since.is_none() {
            self.gap_since = Some(Instant::now());
        }
    }

    /// Drops everything (new stream, decoder reset).
    pub fn clear(&mut self) {
        self.packets.clear();
        self.expected = None;
        self.gap_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_in_sequence() {
        let mut j = JitterBuffer::new();
        let (out, gaps) = j.push(10, vec![10], false);
        assert_eq!(out, vec![(10, vec![10], false)]);
        assert!(gaps.is_empty());
        let (out, gaps) = j.push(12, vec![12], true);
        assert!(out.is_empty());
        assert_eq!(gaps, vec![11]);
        let (out, _) = j.push(11, vec![11], false);
        assert_eq!(out, vec![(11, vec![11], false), (12, vec![12], true)]);
    }

    #[test]
    fn ignores_late_packets() {
        let mut j = JitterBuffer::new();
        j.push(1, vec![1], false);
        j.push(2, vec![2], false);
        let (out, gaps) = j.push(1, vec![9], false);
        assert!(out.is_empty());
        assert!(gaps.is_empty());
    }

    #[test]
    fn resyncs_after_a_big_jump() {
        let mut j = JitterBuffer::new();
        j.push(1, vec![1], false);
        // More than MAX_AHEAD (512): resync instead of buffering forever.
        let (out, gaps) = j.push(600, vec![200], true);
        assert_eq!(out, vec![(600, vec![200], true)]);
        assert!(gaps.is_empty());
    }
}
