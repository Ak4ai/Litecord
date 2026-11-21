//! H.264 encoding through FFmpeg's VA-API backend (Intel/AMD on Linux).
//!
//! The CPU OpenH264 fallback needs ~50 ms for a 1080p picture on a laptop
//! CPU, which caps screen sharing at ~15–20 FPS.  VA-API is a few milliseconds
//! and leaves the CPU free for the call.  We drive the `ffmpeg` binary rather
//! than FFmpeg's C API because the VA-API device/frames setup is a large
//! amount of platform-specific FFI and the CLI already ships on the systems
//! where this path is useful.
//!
//! Frames go in as raw BGRA on stdin and come out as Annex-B H.264 on stdout.
//! `h264_metadata=aud=insert` puts an access-unit delimiter before every
//! picture, so the reader can split complete frames without parsing slice
//! headers.  `dump_extra=freq=keyframe` repeats SPS/PPS on keyframes, which
//! viewers need when they join mid-stream.

use super::VideoEncoder;
use log::{info, warn};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::Duration;

pub struct VaapiFfmpegEncoder {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    frames: Option<Receiver<Vec<u8>>>,
    width: u32,
    height: u32,
    target_fps: u32,
    bitrate_bps: u32,
    device: String,
    failed: bool,
}

impl VaapiFfmpegEncoder {
    /// Checks that FFmpeg and a VA-API render node can really encode H.264.
    /// The check is cheap (one 64x64 picture) and prevents selecting this
    /// backend on machines where only the decoder is available.
    pub fn try_new(target_fps: u32, _is_screen_content: bool) -> Result<Self, String> {
        let device = find_vaapi_render_node()
            .ok_or_else(|| "nenhum nó /dev/dri/renderD* encontrado".to_string())?;

        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-vaapi_device",
                &device,
                "-f",
                "lavfi",
                "-i",
                "color=black:s=64x64:r=1",
                "-frames:v",
                "1",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("ffmpeg não encontrado: {e}"))?;

        if !status.success() {
            return Err(format!(
                "VA-API indisponível em {} (ffmpeg retornou {:?})",
                device,
                status.code()
            ));
        }

        info!("✅ [VAAPI] FFmpeg com aceleração VA-API disponível em {}", device);
        Ok(Self {
            child: None,
            stdin: None,
            frames: None,
            width: 0,
            height: 0,
            target_fps: target_fps.max(1),
            bitrate_bps: 8_000_000,
            device,
            failed: false,
        })
    }

    fn stop(&mut self) {
        self.stdin.take();
        self.frames.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn start(&mut self) {
        self.stop();
        if self.failed || self.width == 0 || self.height == 0 {
            return;
        }

        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(8);
        let mut command = Command::new("ffmpeg");
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-vaapi_device",
            &self.device,
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            &format!("{}x{}", self.width, self.height),
            "-framerate",
            &self.target_fps.to_string(),
            "-i",
            "pipe:0",
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "h264_vaapi",
            "-profile:v",
            "main",
            "-level",
            "4.2",
            "-b:v",
            &self.bitrate_bps.to_string(),
            "-maxrate",
            &self.bitrate_bps.to_string(),
            "-bufsize",
            &(self.bitrate_bps * 2).to_string(),
            "-bf",
            "0",
            "-g",
            &self.target_fps.to_string(),
            "-keyint_min",
            &self.target_fps.to_string(),
            // swscale's BGRA -> NV12 conversion uses BT.601 limited range.
            "-colorspace",
            "smpte170m",
            "-color_range",
            "tv",
            "-bsf:v",
            "h264_metadata=aud=insert,dump_extra=freq=keyframe",
            "-flush_packets",
            "1",
            "-f",
            "h264",
            "pipe:1",
        ]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                warn!("⚠️ [VAAPI] Não foi possível iniciar o ffmpeg: {}", error);
                self.failed = true;
                return;
            }
        };

        let Some(stdin) = child.stdin.take() else {
            self.failed = true;
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            self.failed = true;
            return;
        };

        std::thread::Builder::new()
            .name("vaapi-h264-reader".to_string())
            .spawn(move || read_access_units(stdout, frame_tx))
            .ok();

        info!(
            "🚀 [VAAPI] Encoder FFmpeg VA-API ativo ({}x{} @ {} FPS, {:.1} Mbps, {})",
            self.width,
            self.height,
            self.target_fps,
            self.bitrate_bps as f64 / 1_000_000.0,
            self.device
        );
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.frames = Some(frame_rx);
    }
}

impl Drop for VaapiFfmpegEncoder {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VideoEncoder for VaapiFfmpegEncoder {
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let w = width & !1;
        let h = height & !1;
        if w == 0 || h == 0 || bgra_data.len() < (w * h * 4) as usize {
            return None;
        }

        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.start();
        } else if self.child.is_none() {
            self.start();
        }

        if self.failed {
            return None;
        }

        if let Some(stdin) = self.stdin.as_mut() {
            if stdin
                .write_all(&bgra_data[..(w * h * 4) as usize])
                .is_err()
            {
                self.failed = true;
                return None;
            }
        } else {
            return None;
        }

        // VA-API produces one access unit per input picture.  A short wait is
        // enough; returning None lets the caller keep capturing instead of
        // freezing the UI when the driver hiccups.
        match self.frames.as_ref() {
            Some(rx) => match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(frame) => Some(frame),
                Err(_) => None,
            },
            None => None,
        }
    }

    fn force_intra_frame(&mut self) {
        // The CLI cannot request an IDR without restarting FFmpeg; `-g fps`
        // already gives receivers a keyframe at most one second away.
    }

    fn set_bitrate_bps(&mut self, bitrate_bps: u32) {
        // The CLI reads the bitrate when it starts.  The factory/manager calls
        // this before the first picture, so restarting on a real change is
        // cheap; avoid restarting for tiny ABR adjustments.
        let new = bitrate_bps.max(500_000);
        if self.bitrate_bps.abs_diff(new) > self.bitrate_bps / 5 {
            self.bitrate_bps = new;
            if self.child.is_some() {
                self.start();
            }
        }
    }

    fn get_bitrate_bps(&self) -> u32 {
        self.bitrate_bps
    }

    fn name(&self) -> &'static str {
        "FFmpeg VA-API H.264 (GPU)"
    }

    fn is_hardware_accelerated(&self) -> bool {
        true
    }
}

fn find_vaapi_render_node() -> Option<String> {
    let entries = std::fs::read_dir("/dev/dri").ok()?;
    let mut nodes: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .filter(|path| {
            path.rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

/// Splits FFmpeg's Annex-B output on access-unit delimiters and forwards each
/// complete picture.  The first AUD keeps the SPS/PPS that follow it.
fn read_access_units(mut stdout: impl Read, tx: SyncSender<Vec<u8>>) {
    let mut pending = Vec::with_capacity(512 * 1024);
    let mut chunk = [0u8; 64 * 1024];

    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                loop {
                    let Some(first) = find_aud(&pending, 0) else {
                        break;
                    };
                    if first > 0 {
                        pending.drain(..first);
                    }
                    let Some(second) = find_aud(&pending, 4) else {
                        break;
                    };
                    let frame: Vec<u8> = pending.drain(..second).collect();
                    match tx.try_send(frame) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            // A slow sender must not stall FFmpeg's stdout.
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
            Err(_) => break,
        }
    }

    if pending.len() > 4 {
        let _ = tx.try_send(pending);
    }
}

fn find_aud(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 && buf[i + 3] == 9 {
            return Some(if i > 0 && buf[i - 1] == 0 { i - 1 } else { i });
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::find_aud;

    #[test]
    fn aud_is_found_with_three_and_four_byte_start_codes() {
        assert_eq!(find_aud(&[0, 0, 1, 9, 1], 0), Some(0));
        assert_eq!(find_aud(&[0, 0, 0, 1, 9, 1], 0), Some(0));
    }
}
