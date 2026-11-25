//! MP4 / M4A attachments for the clip player: AAC sound through symphonia's
//! AAC decoder, and H.264 pictures through the openh264 decoder the stream
//! viewer already uses, timed against the sound's clock. Only runs while a
//! clip plays.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mp4::{Mp4Reader, TrackType};
use openh264::formats::YUVSource;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use symphonia_codec_aac::AacDecoder;
use symphonia_core::audio::SampleBuffer;
use symphonia_core::codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_AAC};
use symphonia_core::formats::Packet;

/// Pictures wider than this are scaled down before reaching the UI.
const MAX_WIDTH: usize = 560;

pub fn is_mp4(data: &[u8]) -> bool {
    data.len() > 12 && &data[4..8] == b"ftyp"
}

fn open(data: &[u8]) -> Option<Mp4Reader<Cursor<&[u8]>>> {
    if !is_mp4(data) {
        return None;
    }
    Mp4Reader::read_header(Cursor::new(data), data.len() as u64).ok()
}

/// The sound of an MP4 / M4A file. A video without sound plays silence for its
/// length, so the picture still has a clock to follow.
pub struct AacSource<'a> {
    reader: Mp4Reader<Cursor<&'a [u8]>>,
    track: u32,
    pub next: u32,
    count: u32,
    decoder: Option<AacDecoder>,
    pub rate: u32,
    channels: usize,
    samples: Option<SampleBuffer<f32>>,
    pub duration: f64,
    silent_left: usize,
}

impl<'a> AacSource<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let reader = open(data)?;
        let audio = reader
            .tracks()
            .values()
            .find(|t| t.track_type().ok() == Some(TrackType::Audio) && t.audio_profile().is_ok());
        let duration = reader.duration().as_secs_f64();
        let Some(t) = audio else {
            // Picture only.
            let has_video = reader.tracks().values().any(|t| t.track_type().ok() == Some(TrackType::Video));
            if !has_video {
                return None;
            }
            return Some(AacSource {
                reader,
                track: 0,
                next: 1,
                count: 0,
                decoder: None,
                rate: 48000,
                channels: 2,
                samples: None,
                duration,
                silent_left: (duration * 48000.0) as usize,
            });
        };
        let profile = t.audio_profile().ok()? as u16;
        let freq = t.sample_freq_index().ok()? as u16;
        let chan = t.channel_config().ok()? as u16;
        // AudioSpecificConfig: 5 bits object type, 4 bits rate index, 4 bits channels.
        let asc = (profile << 11) | (freq << 7) | (chan << 3);
        let rate = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]
            .get(freq as usize)
            .copied()
            .unwrap_or(44100);
        let mut params = CodecParameters::new();
        params.for_codec(CODEC_TYPE_AAC).with_sample_rate(rate).with_extra_data(asc.to_be_bytes().to_vec().into_boxed_slice());
        let decoder = AacDecoder::try_new(&params, &DecoderOptions::default()).ok()?;
        let track = t.track_id();
        let count = t.sample_count();
        Some(AacSource {
            reader,
            track,
            next: 1,
            count,
            decoder: Some(decoder),
            rate,
            channels: chan.max(1) as usize,
            samples: None,
            duration,
            silent_left: 0,
        })
    }

    /// Moves to `secs` without decoding what comes before (AAC frames stand alone).
    pub fn jump_to(&mut self, secs: f64) {
        if self.decoder.is_some() {
            self.next = 1 + (secs * self.rate as f64 / 1024.0) as u32;
            if let Some(d) = self.decoder.as_mut() {
                d.reset();
            }
        } else {
            self.silent_left = ((self.duration - secs).max(0.0) * 48000.0) as usize;
        }
    }

    pub fn next_frames(&mut self, out: &mut Vec<[f32; 2]>, chunk: usize) -> bool {
        let Some(decoder) = self.decoder.as_mut() else {
            let n = self.silent_left.min(chunk);
            self.silent_left -= n;
            out.extend(std::iter::repeat_n([0.0, 0.0], n));
            return n > 0;
        };
        while out.len() < chunk {
            if self.next > self.count {
                return !out.is_empty();
            }
            let sample = self.reader.read_sample(self.track, self.next);
            self.next += 1;
            let Ok(Some(sample)) = sample else { continue };
            let packet = Packet::new_from_slice(self.track, sample.start_time, sample.duration as u64, &sample.bytes);
            let Ok(buf) = decoder.decode(&packet) else { continue };
            let spec = *buf.spec();
            let frames = buf.capacity() as u64;
            let sb = self.samples.get_or_insert_with(|| SampleBuffer::new(frames, spec));
            if (sb.capacity() as u64) < frames * spec.channels.count() as u64 {
                *sb = SampleBuffer::new(frames, spec);
            }
            sb.copy_interleaved_ref(buf);
            let ch = spec.channels.count().max(1);
            self.channels = ch;
            for f in sb.samples().chunks_exact(ch) {
                let l = f[0];
                let r = if ch > 1 { f[1] } else { l };
                out.push([l, r]);
            }
        }
        true
    }

    pub fn channels(&self) -> usize {
        self.channels
    }
}

struct VideoSample {
    time: f64,
    sync: bool,
}

/// Shows the video track of `data`, following `clock` (seconds played), until
/// `stop` is set. Jumps on the clock (seeking) are followed from the nearest
/// key frame before them.
pub fn run_video(data: &[u8], clock: impl Fn() -> f64, stop: &AtomicBool, show: impl Fn(SharedPixelBuffer<Rgba8Pixel>)) {
    let Some(mut reader) = open(data) else { return };
    let Some(track) = reader
        .tracks()
        .values()
        .find(|t| t.track_type().ok() == Some(TrackType::Video) && t.sequence_parameter_set().is_ok())
    else {
        return;
    };
    let id = track.track_id();
    let scale = track.timescale().max(1) as f64;
    let count = track.sample_count();
    let mut headers = Vec::new();
    for ps in [track.sequence_parameter_set().unwrap_or(&[]), track.picture_parameter_set().unwrap_or(&[])] {
        headers.extend_from_slice(&[0, 0, 0, 1]);
        headers.extend_from_slice(ps);
    }
    // Flushing after every picture breaks files with B-frames (most phone videos).
    let config = openh264::decoder::DecoderConfig::new().flush_after_decode(openh264::decoder::Flush::NoFlush);
    let Ok(mut decoder) = openh264::decoder::Decoder::with_api_config(openh264::OpenH264API::from_source(), config) else { return };

    // Times and key frames of every picture, for seeking.
    let mut index = Vec::with_capacity(count as usize);
    for n in 1..=count {
        match reader.read_sample(id, n) {
            Ok(Some(s)) => index.push(VideoSample { time: s.start_time as f64 / scale, sync: s.is_sync }),
            _ => index.push(VideoSample { time: f64::MAX, sync: false }),
        }
    }

    let mut next = 0usize; // index into `index`
    let mut last_time = -1.0f64;
    let mut au = Vec::new();
    let mut rgba = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let now = clock();
        // The clock jumped (seek): restart at the key frame before it.
        if last_time >= 0.0 && (now + 0.5 < last_time || now > last_time + 2.0) {
            next = index.iter().rposition(|s| s.sync && s.time <= now).unwrap_or(0);
            last_time = -1.0;
        }
        let Some(entry) = index.get(next) else {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        };
        // Catching up after a jump: decode without showing until the clock.
        let late = entry.time + 0.05 < now;
        if !late && entry.time > now {
            std::thread::sleep(Duration::from_secs_f64((entry.time - now).min(0.02)));
            continue;
        }
        let Ok(Some(sample)) = reader.read_sample(id, next as u32 + 1) else {
            next += 1;
            continue;
        };
        next += 1;
        last_time = entry.time;
        au.clear();
        if entry.sync {
            au.extend_from_slice(&headers);
        }
        // Length-prefixed NAL units to start codes.
        let b = &sample.bytes[..];
        let mut i = 0;
        while i + 4 <= b.len() {
            let n = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            i += 4;
            if i + n > b.len() {
                break;
            }
            au.extend_from_slice(&[0, 0, 0, 1]);
            au.extend_from_slice(&b[i..i + n]);
            i += n;
        }
        let Ok(Some(yuv)) = decoder.decode(&au) else { continue };
        if late && index.get(next).is_some_and(|s| s.time <= now) {
            continue;
        }
        let (w, h) = yuv.dimensions();
        if w == 0 || h == 0 {
            continue;
        }
        rgba.resize(w * h * 4, 0);
        yuv.write_rgba8(&mut rgba);
        show(scale_down(&rgba, w, h));
    }
}

/// Nearest-neighbour shrink to at most MAX_WIDTH wide.
fn scale_down(rgba: &[u8], w: usize, h: usize) -> SharedPixelBuffer<Rgba8Pixel> {
    let (dw, dh) = if w > MAX_WIDTH { (MAX_WIDTH, (h * MAX_WIDTH / w).max(1)) } else { (w, h) };
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(dw as u32, dh as u32);
    let dst = buf.make_mut_bytes();
    for y in 0..dh {
        let sy = y * h / dh;
        for x in 0..dw {
            let sx = x * w / dw;
            let s = (sy * w + sx) * 4;
            let d = (y * dw + x) * 4;
            dst[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a real file: `LITECORD_TEST_MP4=<2 s H.264 + AAC clip>`.
    #[test]
    #[ignore]
    fn decodes_sound_and_pictures_of_a_real_mp4() {
        let data = std::fs::read(std::env::var("LITECORD_TEST_MP4").unwrap()).unwrap();
        let mut src = AacSource::new(&data).expect("mp4 sound");
        let (mut total, mut peak, mut out) = (0, 0.0f32, Vec::new());
        while { out.clear(); src.next_frames(&mut out, 4096) } {
            total += out.len();
            peak = out.iter().fold(peak, |p, f| p.max(f[0].abs()));
        }
        let rate = src.rate as usize;
        assert!(total > rate * 18 / 10 && total < rate * 23 / 10, "{total} frames at {rate}");
        assert!(peak > 0.05, "peak {peak}");

        // Pictures: a clock that follows real time over the clip.
        let start = std::time::Instant::now();
        let stop = AtomicBool::new(false);
        let shown = std::cell::Cell::new(0);
        let clock = || {
            let t = start.elapsed().as_secs_f64();
            if t > 2.2 {
                stop.store(true, Ordering::Relaxed);
            }
            t
        };
        run_video(&data, clock, &stop, |f| {
            assert_eq!(f.width(), 320);
            shown.set(shown.get() + 1);
        });
        assert!(shown.get() > 45, "{} pictures", shown.get());
    }
}
