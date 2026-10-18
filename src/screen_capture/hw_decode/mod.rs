//! H.264 decoding on the GPU. The CPU only parses headers and tracks references
//! (h264_state); the platform backend hands each picture to the GPU's fixed-function
//! decoder, and windows showing the stream draw the picture straight from the GPU
//! (layers). Streams or machines it cannot handle fall back to the CPU decoder.

pub mod h264_state;
#[cfg(windows)]
pub mod device;
#[cfg(windows)]
pub mod dxva;
#[cfg(windows)]
pub mod layers;

use std::collections::HashSet;
use std::sync::Mutex;

use slint::{Rgba8Pixel, SharedPixelBuffer};

#[derive(Debug)]
pub enum HwError {
    /// This stream or machine cannot use the GPU decoder: decode on the CPU.
    Unsupported(String),
    /// Wait for the next keyframe (lost picture, or the stream has not reached one).
    NeedKeyframe(String),
    /// The GPU failed (driver reset, device lost): decode on the CPU.
    Device(String),
}

/// A picture decoded on the GPU.
pub enum GpuPicture {
    /// Drawn straight into the windows that show the stream (its height).
    Shown(u32),
    /// No window shows it on the GPU: copied back as RGBA (the stream's width, height).
    Rgba(SharedPixelBuffer<Rgba8Pixel>, u32, u32),
}

/// Streams currently decoded on the GPU (the UI turns window layers on for them).
static GPU_PEERS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

pub fn peer_on_gpu(peer: u64) -> bool {
    GPU_PEERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.contains(&peer)).unwrap_or(false)
}

fn mark_peer(peer: u64, on: bool) {
    let mut g = GPU_PEERS.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    if on {
        set.insert(peer);
    } else {
        set.remove(&peer);
    }
}

pub struct GpuDecoder {
    peer: u64,
    #[cfg(windows)]
    inner: dxva::DxvaDecoder,
}

impl GpuDecoder {
    pub fn new(peer: u64) -> Result<Self, HwError> {
        #[cfg(windows)]
        {
            Ok(Self { peer, inner: dxva::DxvaDecoder::new()? })
        }
        #[cfg(not(windows))]
        {
            let _ = peer;
            Err(HwError::Unsupported("no GPU decoder on this platform yet".into()))
        }
    }

    /// Decodes one access unit. The newest picture due on screen is drawn into the windows
    /// showing this stream on the GPU, or else copied back at the size `fit(width,
    /// height)` chooses.
    pub fn decode(&mut self, au: &[u8], fit: impl Fn(u32, u32) -> (u32, u32)) -> Result<Option<GpuPicture>, HwError> {
        #[cfg(windows)]
        {
            let Some(frame) = self.inner.decode(au)?.pop() else { return Ok(None) };
            mark_peer(self.peer, true);
            if layers::present(self.peer, &frame, self.inner.shared()) {
                return Ok(Some(GpuPicture::Shown(frame.info.height)));
            }
            let (w, h) = fit(frame.info.width, frame.info.height);
            Ok(Some(GpuPicture::Rgba(self.inner.read_rgba(&frame, w, h)?, frame.info.width, frame.info.height)))
        }
        #[cfg(not(windows))]
        {
            let _ = (au, fit);
            Err(HwError::Unsupported("no GPU decoder on this platform yet".into()))
        }
    }
}

impl Drop for GpuDecoder {
    fn drop(&mut self) {
        mark_peer(self.peer, false);
    }
}
