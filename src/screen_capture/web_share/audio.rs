use crate::screen_capture::stream_audio::{StreamAudioEncoder, FRAME};

/// One shared encoder, allocated only with connected web viewers. The packets are those of
/// the native stream (stereo Opus at 48 kHz, see stream_audio), with RTP timestamps.
pub(super) struct AudioEncoder {
    encoder: StreamAudioEncoder,
    timestamp: u64,
}

impl AudioEncoder {
    pub fn new() -> Option<Self> {
        Some(Self { encoder: StreamAudioEncoder::new()?, timestamp: 0 })
    }

    pub fn push(&mut self, pcm: &[i16], rate: u32) -> Vec<(Vec<u8>, u64)> {
        self.encoder
            .push(pcm, rate)
            .into_iter()
            .map(|packet| {
                let t = self.timestamp;
                self.timestamp += FRAME as u64;
                (packet, t)
            })
            .collect()
    }
}
