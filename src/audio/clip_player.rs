//! Plays audio attachments (Opus voice messages, Ogg Opus/Vorbis, .wav and
//! .mp3) inside the app, with the Opus decoder and cpal output the voice code
//! already uses plus small Vorbis (lewton) and MP3 (minimp3) decoders. Nothing runs until a clip is played: the file is fetched,
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
/// A jump asked for by clicking the progress bar: (generation, 0..1 of the clip).
static SEEK: Mutex<Option<(u64, f64)>> = Mutex::new(None);

/// Jumps the clip playing from `url` to `fraction` (0..1) of its length.
pub fn seek(url: &str, fraction: f64) {
    if *PLAYING_URL.lock().unwrap_or_else(|e| e.into_inner()) == url {
        let gen = GENERATION.load(Ordering::SeqCst);
        *SEEK.lock().unwrap_or_else(|e| e.into_inner()) = Some((gen, fraction.clamp(0.0, 1.0)));
    }
}

fn take_seek(gen: u64) -> Option<f64> {
    let mut s = SEEK.lock().unwrap_or_else(|e| e.into_inner());
    match *s {
        Some((g, f)) if g == gen => {
            *s = None;
            Some(f)
        }
        _ => None,
    }
}

/// Plays the clip, or stops it if it is the one playing.
pub fn toggle(url: String, app_weak: slint::Weak<crate::AppWindow>) {
    if url.is_empty() {
        return;
    }
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if [".flac", ".aac"].iter().any(|e| path.ends_with(e)) {
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
            let ok = play_bytes(&bytes, gen, &app_weak);
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
            if url.is_empty() {
                ui.set_playing_audio_progress(0.0);
                ui.set_playing_audio_time("".into());
                ui.set_playing_video_frame(slint::Image::default());
                ui.set_playing_video_shown(false);
                close_video_window(&ui);
            }
            ui.set_playing_audio_url(url.into());
        }
    });
}

/// Leaves the big video view (and full screen) when the video stops.
pub fn close_video_window(ui: &crate::AppWindow) {
    super::clip_mp4::FULL_SIZE.store(false, Ordering::Relaxed);
    if ui.get_video_modal_open() {
        ui.set_video_modal_open(false);
        if ui.get_video_fullscreen() {
            ui.set_video_fullscreen(false);
            ui.window().set_fullscreen(false);
        }
    }
}

/// "m:ss".
fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Shows how far the clip has played: a bar and "0:12 / 0:45".
fn report_progress(app_weak: &slint::Weak<crate::AppWindow>, gen: u64, played: f64, total: f64) {
    let (progress, text) = if total > 0.0 {
        ((played / total).clamp(0.0, 1.0) as f32, format!("{} / {}", clock(played.min(total)), clock(total)))
    } else {
        (0.0, clock(played))
    };
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if GENERATION.load(Ordering::SeqCst) != gen {
            return;
        }
        if let Some(ui) = w.upgrade() {
            ui.set_playing_audio_progress(progress);
            ui.set_playing_audio_time(text.into());
        }
    });
}

/// The granule position of the last Ogg page: the clip length in samples.
fn ogg_last_granule(bytes: &[u8]) -> Option<u64> {
    let i = bytes.windows(4).rposition(|w| w == b"OggS")?;
    let g = bytes.get(i + 6..i + 14)?;
    Some(u64::from_le_bytes(g.try_into().ok()?))
}

/// Clip length in seconds, 0 when unknown. MP3 is estimated from its bitrate.
fn duration_secs(bytes: &[u8], source: &Source) -> f64 {
    match source {
        Source::Opus(_) => ogg_last_granule(bytes).map_or(0.0, |g| g as f64 / 48000.0),
        Source::Vorbis(v) => ogg_last_granule(bytes).map_or(0.0, |g| g as f64 / v.rate as f64),
        Source::Wav(w) => (w.end - w.pos) as f64 / (w.bits / 8 * w.channels) as f64 / w.rate as f64,
        Source::Mp3(m) if m.bitrate_kbps > 0 => bytes.len() as f64 * 8.0 / (m.bitrate_kbps as f64 * 1000.0),
        Source::Mp3(_) => 0.0,
        Source::Mp4(m) => m.duration,
    }
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
fn open_source(bytes: &[u8]) -> Option<Source<'_>> {
    if let Some(s) = OpusSource::new(bytes) {
        Some(Source::Opus(s))
    } else if let Some(s) = VorbisSource::new(bytes) {
        Some(Source::Vorbis(s))
    } else if let Some(s) = WavSource::new(bytes) {
        Some(Source::Wav(s))
    } else if super::clip_mp4::is_mp4(bytes) {
        super::clip_mp4::AacSource::new(bytes).map(Source::Mp4)
    } else {
        Mp3Source::new(bytes).map(Source::Mp3)
    }
}

/// Opens the clip again and decodes past `target` seconds; returns the source
/// and the frames of the last chunk that lie after the target.
fn source_at(bytes: &[u8], target: f64) -> Option<(Source<'_>, Vec<[f32; 2]>)> {
    let mut fresh = open_source(bytes)?;
    if let Source::Mp4(m) = &mut fresh {
        m.jump_to(target);
        return Some((fresh, Vec::new()));
    }
    let mut skip = (target * fresh.rate() as f64) as usize;
    let mut rest = Vec::new();
    let mut buf = Vec::with_capacity(CHUNK_FRAMES);
    while skip > 0 {
        buf.clear();
        if !fresh.next_frames(&mut buf) {
            break;
        }
        if buf.len() > skip {
            rest.extend_from_slice(&buf[skip..]);
            skip = 0;
        } else {
            skip -= buf.len();
        }
    }
    Some((fresh, rest))
}

fn play_bytes(bytes: &[u8], gen: u64, app_weak: &slint::Weak<crate::AppWindow>) -> bool {
    let Some(source) = open_source(bytes) else { return false };

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

    let total = duration_secs(bytes, &source);
    let (tx, rx) = sync_channel::<Vec<f32>>(4);
    let drained = Arc::new(AtomicBool::new(false));
    let played = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let flush = Arc::new(AtomicBool::new(false));
    let feed = Feed {
        rx,
        cur: Vec::new(),
        pos: 0,
        drained: Arc::clone(&drained),
        played: Arc::clone(&played),
        done: Arc::clone(&done),
        flush: Arc::clone(&flush),
    };
    let stream = match build_stream(&device, &config, feed) {
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
    // Progress goes to the UI at most 4 times a second, only while playing.
    let samples_per_sec = (dst_rate as usize * dst_ch) as f64;
    let mut last_report = std::time::Instant::now() - Duration::from_secs(1);
    let tick = |last: &mut std::time::Instant| {
        if last.elapsed() >= Duration::from_millis(250) {
            *last = std::time::Instant::now();
            report_progress(app_weak, gen, played.load(Ordering::Relaxed) as f64 / samples_per_sec, total);
        }
    };
    // Restarts decoding at `fraction` of the clip: the file is decoded again from
    // the start up to that point (fast next to playback) and the queued audio dropped.
    let jump = |target: f64, rest: Vec<[f32; 2]>, out: &mut Output| {
        out.reset();
        // The output drops what it holds; wait for it so new audio is not dropped too.
        flush.store(true, Ordering::SeqCst);
        let start = std::time::Instant::now();
        while flush.load(Ordering::SeqCst) && start.elapsed() < Duration::from_millis(300) {
            std::thread::sleep(Duration::from_millis(5));
        }
        played.store((target * samples_per_sec) as u64, Ordering::Relaxed);
        done.store(false, Ordering::SeqCst);
        drained.store(false, Ordering::SeqCst);
        out.push(&rest);
    };
    let video_stop = AtomicBool::new(false);
    let has_video = matches!(source, Source::Mp4(_));
    std::thread::scope(|scope| {
    if has_video {
        let played = Arc::clone(&played);
        let video_stop = &video_stop;
        let w = app_weak.clone();
        let _ = std::thread::Builder::new().name("clip-video".into()).spawn_scoped(scope, move || {
            let clock = || played.load(Ordering::Relaxed) as f64 / samples_per_sec;
            let stop = || video_stop.load(Ordering::Relaxed) || GENERATION.load(Ordering::SeqCst) != gen;
            let stop_flag = AtomicBool::new(false);
            let watcher_clock = || {
                if stop() {
                    stop_flag.store(true, Ordering::Relaxed);
                }
                clock()
            };
            super::clip_mp4::run_video(bytes, watcher_clock, &stop_flag, |frame| {
                let w = w.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if GENERATION.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    if let Some(ui) = w.upgrade() {
                        ui.set_playing_video_frame(slint::Image::from_rgba8(frame));
                        ui.set_playing_video_shown(true);
                    }
                });
            });
        });
    }
    let result = (|| {
    loop {
        loop {
            if GENERATION.load(Ordering::SeqCst) != gen {
                return true;
            }
            if let Some(f) = take_seek(gen) {
                if total > 0.0 {
                    if let Some((fresh, rest)) = source_at(bytes, f * total) {
                        source = fresh;
                        jump(f * total, rest, &mut out);
                    }
                }
                last_report = std::time::Instant::now() - Duration::from_secs(1);
            }
            tick(&mut last_report);
            frames.clear();
            if !source.next_frames(&mut frames) {
                break;
            }
            if !out.push(&frames) {
                return true;
            }
        }
        out.flush();
        done.store(true, Ordering::SeqCst);

        // Let the last chunks reach the speaker, unless stopped or moved first.
        let mut moved = false;
        while !drained.load(Ordering::Relaxed) && GENERATION.load(Ordering::SeqCst) == gen {
            if let Some(f) = take_seek(gen) {
                if total > 0.0 {
                    if let Some((fresh, rest)) = source_at(bytes, f * total) {
                        source = fresh;
                        jump(f * total, rest, &mut out);
                    }
                }
                last_report = std::time::Instant::now() - Duration::from_secs(1);
                moved = true;
                break;
            }
            tick(&mut last_report);
            std::thread::sleep(Duration::from_millis(50));
        }
        if !moved {
            break;
        }
    }
    true
    })();
    video_stop.store(true, Ordering::Relaxed);
    result
    });
    drop(stream);
    true
}

fn build_stream(device: &cpal::Device, config: &cpal::SupportedStreamConfig, mut feed: Feed) -> Option<cpal::Stream> {
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
    /// Samples handed to the device so far (counted per chunk).
    played: Arc<AtomicU64>,
    /// Set by the decoder when the clip has been fully decoded.
    done: Arc<AtomicBool>,
    /// Set by the decoder on a jump: drop everything queued.
    flush: Arc<AtomicBool>,
}

impl Feed {
    fn next(&mut self) -> f32 {
        if self.flush.load(Ordering::SeqCst) {
            self.cur.clear();
            self.pos = 0;
            while self.rx.try_recv().is_ok() {}
            self.flush.store(false, Ordering::SeqCst);
        }
        while self.pos >= self.cur.len() {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.played.fetch_add(self.cur.len() as u64, Ordering::Relaxed);
                    self.cur = chunk;
                    self.pos = 0;
                }
                Err(TryRecvError::Empty) => {
                    if self.done.load(Ordering::SeqCst) && !self.drained.swap(true, Ordering::Relaxed) {
                        self.played.fetch_add(self.cur.len() as u64, Ordering::Relaxed);
                        self.cur.clear();
                    }
                    return 0.0;
                }
                Err(TryRecvError::Disconnected) => {
                    if !self.drained.swap(true, Ordering::Relaxed) {
                        self.played.fetch_add(self.cur.len() as u64, Ordering::Relaxed);
                    }
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

    /// Forgets the resampler state and anything not yet sent (after a jump).
    fn reset(&mut self) {
        self.t = 0.0;
        self.last = [0.0; 2];
        self.buf.clear();
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
    Vorbis(VorbisSource<'a>),
    Wav(WavSource<'a>),
    Mp3(Mp3Source<'a>),
    Mp4(super::clip_mp4::AacSource<'a>),
}

impl Source<'_> {
    fn rate(&self) -> u32 {
        match self {
            Source::Opus(_) => 48000,
            Source::Vorbis(v) => v.rate,
            Source::Wav(w) => w.rate,
            Source::Mp3(m) => m.rate,
            Source::Mp4(m) => m.rate,
        }
    }

    /// Appends the next stereo frames; false at the end of the file.
    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        match self {
            Source::Opus(s) => s.next_frames(out),
            Source::Vorbis(s) => s.next_frames(out),
            Source::Wav(s) => s.next_frames(out),
            Source::Mp3(s) => s.next_frames(out),
            Source::Mp4(s) => s.next_frames(out, CHUNK_FRAMES),
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

/// Appends interleaved 16-bit samples as stereo frames.
fn push_i16(out: &mut Vec<[f32; 2]>, data: &[i16], channels: usize) {
    let ch = channels.max(1);
    for f in data.chunks_exact(ch) {
        let l = f[0] as f32 / 32768.0;
        let r = if ch > 1 { f[1] as f32 / 32768.0 } else { l };
        out.push([l, r]);
    }
}

struct VorbisSource<'a> {
    reader: lewton::inside_ogg::OggStreamReader<std::io::Cursor<&'a [u8]>>,
    rate: u32,
    channels: usize,
}

impl<'a> VorbisSource<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        if !data.starts_with(b"OggS") {
            return None;
        }
        let reader = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(data)).ok()?;
        let rate = reader.ident_hdr.audio_sample_rate;
        let channels = reader.ident_hdr.audio_channels as usize;
        (rate > 0 && channels > 0).then_some(Self { reader, rate, channels })
    }

    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        while out.len() < CHUNK_FRAMES {
            match self.reader.read_dec_packet_itl() {
                Ok(Some(pcm)) => push_i16(out, &pcm, self.channels),
                _ => break,
            }
        }
        !out.is_empty()
    }
}

struct Mp3Source<'a> {
    decoder: minimp3::Decoder<std::io::Cursor<&'a [u8]>>,
    rate: u32,
    bitrate_kbps: i32,
    /// The first frame, decoded to learn the rate, waiting to be played.
    first: Option<minimp3::Frame>,
}

impl<'a> Mp3Source<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        // An ID3 tag or an MPEG frame sync, so random files are not guessed as MP3.
        let looks_mp3 = data.starts_with(b"ID3") || (data.len() > 1 && data[0] == 0xFF && data[1] & 0xE0 == 0xE0);
        if !looks_mp3 {
            return None;
        }
        let mut decoder = minimp3::Decoder::new(std::io::Cursor::new(data));
        let first = decoder.next_frame().ok()?;
        let rate = u32::try_from(first.sample_rate).ok().filter(|r| *r > 0)?;
        Some(Self { decoder, rate, bitrate_kbps: first.bitrate, first: Some(first) })
    }

    fn next_frames(&mut self, out: &mut Vec<[f32; 2]>) -> bool {
        if let Some(f) = self.first.take() {
            push_i16(out, &f.data, f.channels);
        }
        while out.len() < CHUNK_FRAMES {
            match self.decoder.next_frame() {
                Ok(f) => push_i16(out, &f.data, f.channels),
                Err(minimp3::Error::SkippedData) => continue,
                Err(_) => break,
            }
        }
        !out.is_empty()
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
    fn clock_and_durations() {
        assert_eq!(clock(75.4), "1:15");
        let data = wav(8000, &[0; 8000]);
        let src = Source::Wav(WavSource::new(&data).unwrap());
        assert!((duration_secs(&data, &src) - 1.0).abs() < 1e-9);
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

    /// Decodes real files: `LITECORD_TEST_DIR` holding 1 s clips `t.ogg`
    /// (Opus), `v.ogg` (Vorbis) and `m.mp3`.
    #[test]
    #[ignore]
    fn decodes_real_vorbis_and_mp3_files() {
        let dir = std::path::PathBuf::from(std::env::var("LITECORD_TEST_DIR").unwrap());
        let v = std::fs::read(dir.join("v.ogg")).unwrap();
        let m = std::fs::read(dir.join("m.mp3")).unwrap();
        assert!(OpusSource::new(&v).is_none());
        let mut srcs = [
            Source::Vorbis(VorbisSource::new(&v).expect("vorbis")),
            Source::Mp3(Mp3Source::new(&m).expect("mp3")),
        ];
        for src in srcs.iter_mut() {
            let rate = src.rate() as usize;
            let (mut total, mut peak, mut out) = (0, 0.0f32, Vec::new());
            while { out.clear(); src.next_frames(&mut out) } {
                total += out.len();
                peak = out.iter().fold(peak, |p, f| p.max(f[0].abs()));
            }
            assert!(total > rate * 9 / 10 && total < rate * 12 / 10, "{total} frames at {rate}");
            assert!(peak > 0.05, "peak {peak}");
        }
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
