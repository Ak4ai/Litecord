//! The audio of a screen share between Litecord clients, as Opus.
//!
//! It went as raw 16-bit PCM: ~768 kbit/s for each viewer, from the sender's upload.
//! Opus at 64 kbit/s carries the same mono mix. The capture runs at the device rate
//! (often 44.1 or 48 kHz); Opus needs 48 kHz, so the encoder resamples linearly. Each
//! packet is one 20 ms frame; a lost one is concealed by the decoder.
//!
//! The mono mix is encoded as two equal channels: opus-rs (0.1.26) encodes mono wrongly
//! (checked against libopus: a tone came out at 22% of its level, a chord at 458%),
//! while its stereo encoder and its decoder are right.

use opus_rs::{Application, OpusDecoder, OpusEncoder};

/// Samples per packet: 20 ms at 48 kHz.
pub const FRAME: usize = 960;
const RATE: u32 = 48_000;
const BITRATE: i32 = 64_000;
/// Gaps longer than this are not concealed (the stream paused or the viewer just joined).
const MAX_CONCEALED: u32 = 3;
/// A packet at most this far behind the last one (one second) is late; further back, the
/// sender started a new stream.
const LATE_WINDOW: u32 = 50;

pub struct StreamAudioEncoder {
    encoder: OpusEncoder,
    pending: Vec<f32>,
    rate: u32,
    /// Position of the next output sample, in input samples, and the last input sample.
    pos: f64,
    last: f32,
}

impl StreamAudioEncoder {
    pub fn new() -> Option<Self> {
        let mut encoder = OpusEncoder::new(RATE as i32, 2, Application::Audio).ok()?;
        encoder.bitrate_bps = BITRATE;
        Some(Self { encoder, pending: Vec::with_capacity(FRAME * 2), rate: 0, pos: 0.0, last: 0.0 })
    }

    /// Mono samples at `rate`; returns the Opus packets that are complete.
    pub fn push(&mut self, pcm: &[i16], rate: u32) -> Vec<Vec<u8>> {
        if !(8_000..=192_000).contains(&rate) {
            return Vec::new();
        }
        if rate != self.rate {
            self.rate = rate;
            self.pending.clear();
            self.pos = 0.0;
            self.last = 0.0;
        }
        if rate == RATE {
            self.pending.extend(pcm.iter().map(|&s| s as f32 / 32768.0));
        } else {
            // Linear interpolation between the previous sample (index -1) and this block.
            let step = rate as f64 / RATE as f64;
            let at = |i: isize| -> f32 { if i < 0 { self.last } else { pcm[i as usize] as f32 / 32768.0 } };
            while self.pos < pcm.len() as f64 - 1.0 {
                let i = self.pos.floor() as isize;
                let frac = (self.pos - i as f64) as f32;
                let s = at(i) * (1.0 - frac) + at(i + 1) * frac;
                self.pending.push(s);
                self.pos += step;
            }
            self.pos -= pcm.len() as f64;
            if let Some(&l) = pcm.last() {
                self.last = l as f32 / 32768.0;
            }
        }
        let mut out = Vec::new();
        while self.pending.len() >= FRAME {
            let stereo: Vec<f32> = self.pending[..FRAME].iter().flat_map(|&s| [s, s]).collect();
            let mut packet = vec![0u8; 1500];
            if let Ok(len) = self.encoder.encode(&stereo, FRAME, &mut packet) {
                packet.truncate(len);
                out.push(packet);
            }
            self.pending.drain(..FRAME);
        }
        out
    }
}

pub struct StreamAudioDecoder {
    decoder: OpusDecoder,
    last_seq: Option<u32>,
    last_toc: Option<u8>,
}

impl StreamAudioDecoder {
    pub fn new() -> Option<Self> {
        // Stereo, like the packets (opus-rs cannot decode them into mono).
        Some(Self { decoder: OpusDecoder::new(RATE as i32, 2).ok()?, last_seq: None, last_toc: None })
    }

    /// The 48 kHz samples of packet `seq`, after concealing up to a few lost ones before it.
    pub fn decode(&mut self, seq: u32, packet: &[u8]) -> Vec<i16> {
        let mut out = Vec::new();
        if let (Some(last), Some(toc)) = (self.last_seq, self.last_toc) {
            let missing = seq.wrapping_sub(last).wrapping_sub(1);
            if missing > 0 && missing <= MAX_CONCEALED {
                for _ in 0..missing {
                    // A packet of only its ToC byte asks the decoder to conceal a lost frame.
                    self.decode_into(&[toc], &mut out);
                }
            }
        }
        if let Some(last) = self.last_seq {
            let behind = last.wrapping_sub(seq);
            if behind < LATE_WINDOW {
                return out; // late or repeated packet
            }
            if behind <= u32::MAX / 2 {
                // Far behind: the sender started a new stream and counts from the start again.
                if let Some(fresh) = StreamAudioDecoder::new() {
                    *self = fresh;
                }
            }
        }
        self.last_seq = Some(seq);
        if let Some(&toc) = packet.first() {
            self.last_toc = Some(toc);
            self.decode_into(packet, &mut out);
        }
        out
    }

    fn decode_into(&mut self, packet: &[u8], out: &mut Vec<i16>) {
        let mut pcm = [0f32; FRAME * 2];
        if let Ok(n) = self.decoder.decode(packet, FRAME, &mut pcm) {
            // Both channels carry the same mix: keep the left one.
            out.extend(pcm[..n.min(FRAME) * 2].iter().step_by(2).map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, freq: f32, seconds: f32) -> Vec<i16> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| ((i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * 12000.0) as i16)
            .collect()
    }

    /// Frequency from zero crossings, skipping the codec's start-up.
    fn frequency(pcm: &[i16]) -> f32 {
        let s = &pcm[pcm.len() / 4..];
        let crossings = s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        crossings as f32 / 2.0 / (s.len() as f32 / RATE as f32)
    }

    fn round_trip(rate: u32) -> (Vec<i16>, usize) {
        let mut enc = StreamAudioEncoder::new().unwrap();
        let mut dec = StreamAudioDecoder::new().unwrap();
        let input = tone(rate, 440.0, 1.0);
        let (mut out, mut bytes) = (Vec::new(), 0);
        // Blocks of 10 ms, as the loopback capture delivers them.
        for (seq, block) in input.chunks((rate / 100) as usize).enumerate() {
            let _ = seq;
            for p in enc.push(block, rate) {
                bytes += p.len();
                let n = (out.len() / FRAME) as u32;
                out.extend(dec.decode(n, &p));
            }
        }
        (out, bytes)
    }

    #[test]
    fn a_tone_comes_back_at_its_pitch_from_44_1_and_48_khz() {
        for rate in [44_100, 48_000] {
            let (out, bytes) = round_trip(rate);
            // About a second of audio, at 48 kHz.
            assert!(out.len() >= RATE as usize * 9 / 10, "{rate}: {} samples", out.len());
            let f = frequency(&out);
            assert!((f - 440.0).abs() < 8.0, "{rate}: {f} Hz");
            // ~64 kbit/s instead of 768 kbit/s of PCM.
            assert!(bytes * 8 < 90_000, "{rate}: {} bit/s", bytes * 8);
            let peak = out[out.len() / 2..].iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 8000, "{rate}: peak {peak}");
        }
    }

    fn rms(x: &[i16]) -> f32 {
        (x.iter().map(|&v| (v as f32) * (v as f32)).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// The decoder was checked against libopus, so a round trip shows what the encoder
    /// does to the level: a tone, a chord and a voice-like signal keep theirs.
    #[test]
    fn levels_survive_the_round_trip() {
        let tau = std::f32::consts::TAU;
        let at = |i: usize| i as f32 / 48_000.0;
        let signals: [(&str, Vec<i16>); 3] = [
            ("tone", (0..48_000).map(|i| ((at(i) * 440.0 * tau).sin() * 12000.0) as i16).collect()),
            ("chord", (0..48_000).map(|i| (4000.0 * [261.6f32, 329.6, 392.0].iter().map(|f| (at(i) * f * tau).sin()).sum::<f32>()) as i16).collect()),
            ("voice", (0..48_000).map(|i| ((at(i) * 4.0 * tau).sin().abs() * 9000.0 * ((at(i) * 150.0 * tau).sin() * 0.6 + (at(i) * 600.0 * tau).sin() * 0.3 + (at(i) * 2400.0 * tau).sin() * 0.1)) as i16).collect()),
        ];
        for (name, input) in signals {
            let mut enc = StreamAudioEncoder::new().unwrap();
            let mut dec = StreamAudioDecoder::new().unwrap();
            let mut out = Vec::new();
            for (n, p) in input.chunks(480).flat_map(|c| enc.push(c, 48_000)).enumerate() {
                out.extend(dec.decode(n as u32, &p));
            }
            let level = rms(&out[9600..]) / rms(&input[9600..out.len()]);
            assert!((0.8..1.25).contains(&level), "{name}: level {level:.2}");
        }
    }

    #[test]
    fn a_lost_packet_is_concealed_with_a_frame_of_audio() {
        let mut enc = StreamAudioEncoder::new().unwrap();
        let mut dec = StreamAudioDecoder::new().unwrap();
        let packets: Vec<_> = tone(48_000, 440.0, 0.2).chunks(FRAME).flat_map(|c| enc.push(c, 48_000)).collect();
        assert!(packets.len() >= 8);
        assert_eq!(dec.decode(0, &packets[0]).len(), FRAME);
        assert_eq!(dec.decode(1, &packets[1]).len(), FRAME);
        // Packet 2 lost: packet 3 brings a concealed frame before its own.
        assert_eq!(dec.decode(3, &packets[3]).len(), 2 * FRAME);
        // A long gap (the stream paused) is not filled.
        assert_eq!(dec.decode(50, &packets[4]).len(), FRAME);
        // A late packet is dropped.
        assert_eq!(dec.decode(49, &packets[5]).len(), 0);
    }

    #[test]
    fn a_new_stream_from_the_same_peer_starts_counting_again() {
        let mut enc = StreamAudioEncoder::new().unwrap();
        let mut dec = StreamAudioDecoder::new().unwrap();
        let packets: Vec<_> = tone(48_000, 440.0, 0.1).chunks(FRAME).flat_map(|c| enc.push(c, 48_000)).collect();
        assert_eq!(dec.decode(5000, &packets[0]).len(), FRAME);
        // The sender stopped and started again: its packets count from 1.
        assert_eq!(dec.decode(1, &packets[1]).len(), FRAME);
        assert_eq!(dec.decode(2, &packets[2]).len(), FRAME);
    }

    /// The packets play right in libopus (what browsers use): needs ffmpeg with libopus.
    /// cargo test --release --lib stream_audio -- --ignored
    #[test]
    #[ignore]
    fn libopus_plays_the_stream_at_its_level() {
        use std::process::Command;
        let dir = std::env::temp_dir().join("litecord-opus-check");
        std::fs::create_dir_all(&dir).unwrap();
        let input: Vec<i16> = (0..96_000).map(|i| (4000.0 * [261.6f32, 329.6, 392.0].iter().map(|f| (i as f32 / 48_000.0 * f * std::f32::consts::TAU).sin()).sum::<f32>()) as i16).collect();
        let mut enc = StreamAudioEncoder::new().unwrap();
        let packets: Vec<_> = input.chunks(480).flat_map(|c| enc.push(c, 48_000)).collect();
        let ogg = dir.join("stream.ogg");
        std::fs::write(&ogg, ogg_opus(&packets, 2)).unwrap();
        let raw = dir.join("stream.s16");
        let ok = Command::new("ffmpeg").args(["-y", "-loglevel", "error", "-c:a", "libopus", "-i"]).arg(&ogg).args(["-f", "s16le", "-ac", "2", "-ar", "48000"]).arg(&raw).status();
        let Ok(status) = ok else { eprintln!("ffmpeg not found"); return };
        assert!(status.success());
        let left: Vec<i16> = std::fs::read(&raw).unwrap().chunks_exact(4).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let level = rms(&left[9600..input.len().min(left.len())]) / rms(&input[9600..]);
        assert!((0.8..1.25).contains(&level), "libopus level {level:.2}");
    }

    /// A minimal Ogg Opus file (RFC 7845), one packet per page.
    fn ogg_opus(packets: &[Vec<u8>], channels: u8) -> Vec<u8> {
        fn crc(data: &[u8]) -> u32 {
            let mut c = 0u32;
            for &b in data {
                c ^= (b as u32) << 24;
                for _ in 0..8 {
                    c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04c1_1db7 } else { c << 1 };
                }
            }
            c
        }
        fn page(out: &mut Vec<u8>, flag: u8, granule: u64, seq: u32, packet: &[u8]) {
            let mut lacing = vec![255u8; packet.len() / 255];
            lacing.push((packet.len() % 255) as u8);
            let mut p = b"OggS\0".to_vec();
            p.push(flag);
            p.extend_from_slice(&granule.to_le_bytes());
            p.extend_from_slice(&7u32.to_le_bytes());
            p.extend_from_slice(&seq.to_le_bytes());
            p.extend_from_slice(&[0; 4]);
            p.push(lacing.len() as u8);
            p.extend_from_slice(&lacing);
            p.extend_from_slice(packet);
            let c = crc(&p);
            p[22..26].copy_from_slice(&c.to_le_bytes());
            out.extend(p);
        }
        let mut out = Vec::new();
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, channels, 0, 0]);
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&[0, 0, 0]);
        page(&mut out, 2, 0, 0, &head);
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&4u32.to_le_bytes());
        tags.extend_from_slice(b"test");
        tags.extend_from_slice(&0u32.to_le_bytes());
        page(&mut out, 0, 0, 1, &tags);
        for (k, p) in packets.iter().enumerate() {
            page(&mut out, if k + 1 == packets.len() { 4 } else { 0 }, ((k + 1) * FRAME) as u64, k as u32 + 2, p);
        }
        out
    }
}
