//! Plays audio attachments (Opus voice messages, .ogg/.opus files and .wav)
//! inside the app, with the Opus decoder and cpal output the voice code
//! already uses. Nothing runs until a clip is played: the file is fetched,
//! decoded a little ahead of the speaker on its own thread, and the thread and
//! output stream go away when the clip ends or is stopped. Other formats are
//! opened outside the app, as before.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{info, warn};
use slint::ComponentHandle;

/// Bigger files are opened outside the app instead of being downloaded here.
const MAX_BYTES: usize = 50 * 1024 * 1024;
/// Device frames per chunk handed to the output callback (~85 ms at 48 kHz).
const CHUNK_FRAMES: usize = 4096;

/// Bumped on every play or stop; a running clip quits when it no longer matches.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static PLAYING_URL: Mutex<String> = Mutex::new(String::new());

/// Plays the clip, or stops it if it is the one playing.
pub fn toggle(url: String, app_weak: slint::Weak<crate::AppWindow>) {
    if url.is_empty() {
        return;
    }
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if [".mp3", ".m4a", ".flac", ".aac"].iter().any(|e| path.ends_with(e)) {
        // Formats this player does not decode: no point downloading them here.
        let w = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                ui.invoke_open_link(url.into());
            }
        });
        return;
    }
    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let was_playing = {
        let mut cur = PLAYING_URL.lock().unwrap_or_else(|e| e.into_inner());
        let same = *cur == url;
        *cur = if same { String::new() } else { url.clone() };
        same
    };
    if was_playing {
        set_playing_ui(&app_weak, String::new());
        return;
    }
    set_playing_ui(&app_weak, url.clone());

    tokio::spawn(async move {
        let bytes = match download(&url).await {
            Some(b) => b,
            None => return fall_back(gen, url, app_weak),
        };
        if GENERATION.load(Ordering::SeqCst) != gen {
            return;
        }
        let _ = std::thread::Builder::new().name("clip-player".into()).spawn(move || {
            let ok = play_bytes(&bytes, gen);
            if !ok {
                fall_back(gen, url, app_weak);
            } else {
                finish(gen, &app_weak);
            }
        });
    });
}

/// Stops whatever is playing (e.g. when leaving the channel).
pub fn stop(app_weak: &slint::Weak<crate::AppWindow>) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    PLAYING_URL.lock().unwrap_or_else(|e| e.into_inner()).clear();
    set_playing_ui(app_weak, String::new());
}

fn set_playing_ui(app_weak: &slint::Weak<crate::AppWindow>, url: String) {
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = w.upgrade() {
            ui.set_playing_audio_url(url.into());
        }
    });
}

/// Clears the playing state if this clip is still the current one.
fn finish(gen: u64, app_weak: &slint::Weak<crate::AppWindow>) {
    if GENERATION.load(Ordering::SeqCst) == gen {
        PLAYING_URL.lock().unwrap_or_else(|e| e.into_inner()).clear();
        set_playing_ui(app_weak, String::new());
    }
}

/// The file could not be played here: open it outside the app instead.
fn fall_back(gen: u64, url: String, app_weak: slint::Weak<crate::AppWindow>) {
    if GENERATION.load(Ordering::SeqCst) != gen {
        return;
    }
    finish(gen, &app_weak);
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = w.upgrade() {
            ui.invoke_open_link(url.into());
        }
    });
}

async fn download(url: &str) -> Option<Vec<u8>> {
    let builder = reqwest::Client::builder().timeout(Duration::from_secs(30));
    let client = crate::utils::apply_proxy_to_builder(builder).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|l| l as usize > MAX_BYTES) {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    (bytes.len() <= MAX_BYTES).then(|| bytes.to_vec())
}

/// Decodes and plays the file. Returns false when the format is not one this
/// player reads or no output device opens.
fn play_bytes(bytes: &[u8], gen: u64) -> bool {
    let source = if let Some(s) = OpusSource::new(bytes) {
        Source::Opus(s)
    } else if let Some(s) = WavSource::new(bytes) {
        Source::Wav(s)
    } else {
        return false;
    };

    let host = cpal::default_host();
    let wanted = crate::gateway::get_selected_output_device_store()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let device = if crate::utils::i18n_extra::is_default_device(&wanted) {
        host.default_output_device()
    } else {
        host.output_devices()
            .ok()
            .and_then(|mut d| d.find(|d| d.name().is_ok_and(|n| n == wanted)))
            .or_else(|| host.default_output_device())
    };
    let Some(device) = device else { return false };
    let Some(config) = crate::audio::engine::select_preferred_48khz_config(&device, false) else { return false };
    let dst_rate = config.sample_rate().0;
    let dst_ch = config.channels() as usize;

    let (tx, rx) = sync_channel::<Vec<f32>>(4);
    let drained = Arc::new(AtomicBool::new(false));
    let stream = match build_stream(&device, &config, rx, Arc::clone(&drained)) {
        Some(s) => s,
        None => return false,
    };
    if stream.play().is_err() {
        return false;
    }
    info!("🔊 [CLIP] Tocando anexo de áudio ({} Hz, {} canais)", dst_rate, dst_ch);

    let mut out = Output::new(tx, source.rate(), dst_rate, dst_ch);
    let mut source = source;
    let mut frames = Vec::with_capacity(CHUNK_FRAMES);
    loop {
        if GENERATION.load(Ordering::SeqCst) != gen {
            return true;
        }
        frames.clear();
        if !source.next_frames(&mut frames) {
            break;
        }
        if !out.push(&frames) {
            return true;
        }
    }
    out.flush();
    drop(out);

    // Let the last chunks reach the speaker, unless stopped first.
    while !drained.load(Ordering::Relaxed) && GENERATION.load(Ordering::SeqCst) == gen {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    true
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    rx: Receiver<Vec<f32>>,
    drained: Arc<AtomicBool>,
) -> Option<cpal::Stream> {
    let mut feed = Feed { rx, cur: Vec::new(), pos: 0, drained };
    let err = |e| warn!("Erro no stream do player de áudio: {}", e);
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_output_stream(&config.config(), move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for s in data.iter_mut() {
                    *s = feed.next();
                }
            }, err, None)
            .ok(),
        cpal::SampleFormat::I16 => device
            .build_output_stream(&config.config(), move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for s in data.iter_mut() {
                    *s = (feed.next() * 32767.0) as i16;
                }
            }, err, None)
            .ok(),
        _ => None,
    }
}

/// Output-callback side: hands out samples from the decoded chunks.
struct Feed {
    rx: Receiver<Vec<f32>>,
    cur: Vec<f32>,
    pos: usize,
    drained: Arc<AtomicBool>,
}

impl Feed {
    fn next(&mut self) -> f32 {
        while self.pos >= self.cur.len() {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.cur = chunk;
                    self.pos = 0;
                }
                Err(TryRecvError::Empty) => return 0.0,
                Err(TryRecvError::Disconnected) => {
                    self.drained.store(true, Ordering::Relaxed);
                    return 0.0;
                }
            }
        }
        let s = self.cur[self.pos];
        self.pos += 1;
        s.clamp(-1.0, 1.0)
    }
}

/// Decoder side: resamples stereo frames to the device rate and channel count
/// (linear interpolation) and sends them in chunks.
struct Output {
    tx: SyncSender<Vec<f32>>,
    step: f64,
    t: f64,
    last: [f32; 2],
    dst_ch: usize,
    buf: Vec<f32>,
}

impl Output {
    fn new(tx: SyncSender<Vec<f32>>, src_rate: u32, dst_rate: u32, dst_ch: usize) -> Self {
        Self {
            tx,
            step: src_rate as f64 / dst_rate.max(1) as f64,
            t: 0.0,
            last: [0.0; 2],
            dst_ch: dst_ch.max(1),
            buf: Vec::with_capacity(CHUNK_FRAMES * dst_ch.max(1)),
        }
    }

    /// False once the output side is gone.
    fn push(&mut self, frames: &[[f32; 2]]) -> bool {
        for &f in frames {
            while self.t < 1.0 {
                let t = self.t as f32;
                let l = self.last[0] + (f[0] - self.last[0]) * t;
                let r = self.last[1] + (f[1] - self.last[1]) * t;
                match self.dst_ch {
                    1 => self.buf.push((l + r) * 0.5),
                    n => {
                        self.buf.push(l);
                        self.buf.push(r);
                        self.buf.extend(std::iter::repeat(0.0).take(n - 2));
                    }
                }
                self.t += self.step;
            }
            self.t -= 1.0;
            self.last = f;
        }
        if self.buf.len() >= CHUNK_FRAMES * self.dst_ch {
            return self.flush();
        }
        true
    }

    fn flush(&mut self) -> bool {
        if self.buf.is_empty() {
            return true;
        }
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK_FRAMES * self.dst_ch));
        self.tx.send(chunk).is_ok()
    }
}

enum Source<'a> {
    Opus(OpusSource<'a>),
    Wav(WavSource<'a>),
}

impl Source<'_> {
    fn rate(&self) -> u32 {
        match self {
            Source::Opus(_) => 48000,
            Source::Wav(w) => w.rate,
        }
    }

    /// Appends the next stereo frames; false at the end of the file.
    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        match self {
            Source::Opus(s) => s.next_frames(out),
            Source::Wav(s) => s.next_frames(out),
        }
    }
}

/// Reads Ogg pages one packet at a time.
struct OggPackets<'a> {
    data: &'a [u8],
    pos: usize,
    segs: &'a [u8],
    seg_idx: usize,
}

impl<'a> OggPackets<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, segs: &[], seg_idx: 0 }
    }

    fn next_packet(&mut self, packet: &mut Vec<u8>) -> bool {
        packet.clear();
        loop {
            while self.seg_idx < self.segs.len() {
                let len = self.segs[self.seg_idx] as usize;
                self.seg_idx += 1;
                let Some(body) = self.data.get(self.pos..self.pos + len) else { return false };
                packet.extend_from_slice(body);
                self.pos += len;
                if len < 255 {
                    return true;
                }
            }
            // Next page header.
            let Some(head) = self.data.get(self.pos..self.pos + 27) else { return false };
            if &head[..4] != b"OggS" {
                return false;
            }
            let nseg = head[26] as usize;
            let Some(segs) = self.data.get(self.pos + 27..self.pos + 27 + nseg) else { return false };
            self.segs = segs;
            self.seg_idx = 0;
            self.pos += 27 + nseg;
        }
    }
}

struct OpusSource<'a> {
    packets: OggPackets<'a>,
    decoder: opus_rs::OpusDecoder,
    channels: usize,
    skip: usize,
    pcm: Vec<f32>,
    packet: Vec<u8>,
}

impl<'a> OpusSource<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let mut packets = OggPackets::new(data);
        let mut head = Vec::new();
        if !packets.next_packet(&mut head) || head.len() < 19 || &head[..8] != b"OpusHead" {
            return None;
        }
        let channels = head[9] as usize;
        if channels != 1 && channels != 2 {
            return None;
        }
        let skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        let mut tags = Vec::new();
        packets.next_packet(&mut tags); // OpusTags
        let decoder = opus_rs::OpusDecoder::new(48000, channels).ok()?;
        Some(Self { packets, decoder, channels, skip, pcm: vec![0.0; 5760 * 2], packet: Vec::new() })
    }

    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        while out.len() < CHUNK_FRAMES {
            if !self.packets.next_packet(&mut self.packet) {
                return !out.is_empty();
            }
            if self.packet.is_empty() {
                continue;
            }
            let Ok(n) = self.decoder.decode(&self.packet, 5760, &mut self.pcm[..]) else { continue };
            for i in 0..n {
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                out.push(if self.channels == 2 {
                    [self.pcm[i * 2], self.pcm[i * 2 + 1]]
                } else {
                    [self.pcm[i]; 2]
                });
            }
        }
        true
    }
}

struct WavSource<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
    rate: u32,
    channels: usize,
    /// 1 = PCM integer, 3 = float.
    format: u16,
    bits: usize,
}

impl<'a> WavSource<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return None;
        }
        let u16_at = |p: usize| data.get(p..p + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let u32_at = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let mut p = 12;
        let mut fmt = None;
        while p + 8 <= data.len() {
            let id = &data[p..p + 4];
            let len = u32_at(p + 4)? as usize;
            let body = p + 8;
            if id == b"fmt " {
                let mut format = u16_at(body)?;
                if format == 0xFFFE {
                    format = u16_at(body + 24)?; // WAVE_FORMAT_EXTENSIBLE sub-format
                }
                fmt = Some((format, u16_at(body + 2)? as usize, u32_at(body + 4)?, u16_at(body + 14)? as usize));
            } else if id == b"data" {
                let (format, channels, rate, bits) = fmt?;
                let ok = channels >= 1
                    && rate > 0
                    && matches!((format, bits), (1, 8) | (1, 16) | (1, 24) | (1, 32) | (3, 32));
                if !ok {
                    return None;
                }
                let end = (body + len).min(data.len());
                return Some(Self { data, pos: body, end, rate, channels, format, bits });
            }
            p = body + len + (len & 1);
        }
        None
    }

    fn sample(&self, at: usize) -> f32 {
        let b = &self.data[at..at + self.bits / 8];
        match (self.format, self.bits) {
            (1, 8) => (b[0] as f32 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        let width = self.bits / 8;
        let frame = width * self.channels;
        while out.len() < CHUNK_FRAMES && self.pos + frame <= self.end {
            let l = self.sample(self.pos);
            let r = if self.channels > 1 { self.sample(self.pos + width) } else { l };
            out.push([l, r]);
            self.pos += frame;
        }
        !out.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * 2).to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(((samples.len() * 2) as u32).to_le_bytes());
        for s in samples {
            v.extend(s.to_le_bytes());
        }
        v
    }

    #[test]
    fn reads_a_mono_wav() {
        let data = wav(22050, &[0, 16384, -16384]);
        let mut src = WavSource::new(&data).expect("wav");
        assert_eq!(src.rate, 22050);
        let mut out = Vec::new();
        assert!(src.next_frames(&mut out));
        assert_eq!(out, vec![[0.0, 0.0], [0.5, 0.5], [-0.5, -0.5]]);
        out.clear();
        assert!(!src.next_frames(&mut out));
    }

    #[test]
    fn rejects_files_it_cannot_read() {
        assert!(WavSource::new(b"ID3\x03mp3 data here").is_none());
        assert!(OpusSource::new(b"OggS not really").is_none());
    }

    #[test]
    fn resamples_to_the_device_rate() {
        let (tx, rx) = sync_channel(8);
        let mut out = Output::new(tx, 24000, 48000, 2);
        assert!(out.push(&[[1.0, -1.0]; 100]));
        assert!(out.flush());
        let chunk = rx.try_recv().unwrap();
        assert_eq!(chunk.len(), 200 * 2);
    }

    /// Decodes a real Ogg Opus file: `LITECORD_TEST_OGG=<1 s mono clip>`.
    #[test]
    #[ignore]
    fn decodes_a_real_ogg_opus_file() {
        let data = std::fs::read(std::env::var("LITECORD_TEST_OGG").unwrap()).unwrap();
        let mut src = OpusSource::new(&data).expect("opus");
        let mut total = 0;
        let mut peak = 0.0f32;
        let mut out = Vec::new();
        while { out.clear(); src.next_frames(&mut out) } {
            total += out.len();
            peak = out.iter().fold(peak, |p, f| p.max(f[0].abs()));
        }
        assert!((47_000..=49_000).contains(&total), "{total} frames");
        assert!(peak > 0.1, "peak {peak}");
    }

    #[test]
    fn splits_ogg_packets_across_segments() {
        // One page holding a 300-byte packet (255 + 45) and a 3-byte one.
        let mut page = b"OggS".to_vec();
        page.extend([0u8; 22]);
        page.push(3);
        page.extend([255, 45, 3]);
        page.extend(vec![7u8; 300]);
        page.extend([1, 2, 3]);
        let mut pk = OggPackets::new(&page);
        let mut p = Vec::new();
        assert!(pk.next_packet(&mut p));
        assert_eq!(p.len(), 300);
        assert!(pk.next_packet(&mut p));
        assert_eq!(p, vec![1, 2, 3]);
        assert!(!pk.next_packet(&mut p));
    }
}
