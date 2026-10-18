use opus_rs::{Application, OpusEncoder};
/// One shared mono encoder, allocated only with connected web viewers.
pub(super) struct AudioEncoder {
    encoder: OpusEncoder,
    pending: Vec<f32>,
    phase: u64,
    timestamp: u64,
    rate: u32,
}
impl AudioEncoder {
    pub fn new() -> Option<Self> {
        Some(Self { encoder: OpusEncoder::new(48000, 1, Application::Audio).ok()?,
            pending: Vec::with_capacity(1920), phase: 0, timestamp: 0, rate: 0 })
    }
    pub fn push(&mut self, pcm: &[i16], rate: u32) -> Vec<(Vec<u8>, u64)> {
        if !(8000..=192000).contains(&rate) { return Vec::new(); }
        if self.rate != rate { self.pending.clear(); self.phase = 0; self.rate = rate; }
        for &s in pcm {
            self.phase += 48000;
            while self.phase >= rate as u64 {
                self.phase -= rate as u64;
                self.pending.push(s as f32 / 32768.0);
            }
        }
        let mut out = Vec::new();
        while self.pending.len() >= 960 {
            let mut packet = vec![0; 4000];
            if let Ok(len) = self.encoder.encode(&self.pending[..960], 960, &mut packet) {
                packet.truncate(len);
                out.push((packet, self.timestamp));
            }
            self.timestamp += 960;
            self.pending.drain(..960);
        }
        out
    }
}
