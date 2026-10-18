//! H.264 decoding on the GPU. The CPU only parses headers and tracks references
//! (h264_state); the platform backend hands each picture to the GPU's fixed-function
//! decoder. Streams or machines it cannot handle fall back to the CPU decoder.

pub mod h264_state;
#[cfg(windows)]
pub mod dxva;

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

pub struct GpuDecoder {
    #[cfg(windows)]
    inner: dxva::DxvaDecoder,
}

impl GpuDecoder {
    pub fn new() -> Result<Self, HwError> {
        #[cfg(windows)]
        {
            Ok(Self { inner: dxva::DxvaDecoder::new()? })
        }
        #[cfg(not(windows))]
        {
            Err(HwError::Unsupported("no GPU decoder on this platform yet".into()))
        }
    }

    /// Decodes one access unit and returns the newest picture due on screen as RGBA, at
    /// the size `fit(width, height)` chooses.
    pub fn decode_rgba(&mut self, au: &[u8], fit: impl Fn(u32, u32) -> (u32, u32)) -> Result<Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)>, HwError> {
        #[cfg(windows)]
        {
            let Some(frame) = self.inner.decode(au)?.pop() else { return Ok(None) };
            let (w, h) = fit(frame.info.width, frame.info.height);
            Ok(Some((self.inner.read_rgba(&frame, w, h)?, w, h)))
        }
        #[cfg(not(windows))]
        {
            let _ = (au, fit);
            Err(HwError::Unsupported("no GPU decoder on this platform yet".into()))
        }
    }
}
