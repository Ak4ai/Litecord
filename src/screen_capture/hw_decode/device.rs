//! The D3D11 device shared by the GPU decoder and the windows that show its pictures (a
//! decoded picture can only be shown by the device that decoded it). Created on first use
//! on the adapter that drives the screen, and released when nothing holds it any more.

use std::sync::{Arc, Mutex, Weak};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

use super::HwError;

pub struct SharedDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

// The immediate context is multithread-protected (set below), so the decoder thread and
// the UI thread can both use it.
unsafe impl Send for SharedDevice {}
unsafe impl Sync for SharedDevice {}

impl Drop for SharedDevice {
    fn drop(&mut self) {
        log::info!("🎞️ [GPU VIDEO] Dispositivo da GPU liberado (ninguém assiste pela GPU)");
    }
}

static DEVICE: Mutex<Weak<SharedDevice>> = Mutex::new(Weak::new());

pub fn shared_device() -> Result<Arc<SharedDevice>, HwError> {
    let mut slot = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = slot.upgrade() {
        return Ok(d);
    }
    let d = Arc::new(unsafe { create()? });
    log::info!("🎞️ [GPU VIDEO] Dispositivo da GPU criado");
    *slot = Arc::downgrade(&d);
    Ok(d)
}

unsafe fn create() -> Result<SharedDevice, HwError> {
    let adapter = display_adapter()?;
    let mut device = None;
    let mut context = None;
    D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, None, D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
    let device: ID3D11Device = device.ok_or(HwError::Unsupported("no D3D11 device".into()))?;
    let context: ID3D11DeviceContext = context.ok_or(HwError::Unsupported("no D3D11 context".into()))?;
    let mt: ID3D11Multithread = context.cast()?;
    let _ = mt.SetMultithreadProtected(true);
    Ok(SharedDevice { device, context })
}

/// The adapter with a screen attached (on a laptop with two GPUs, the one the window is
/// shown on), so pictures never cross between GPUs.
unsafe fn display_adapter() -> Result<IDXGIAdapter1, HwError> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let mut i = 0;
    while let Ok(a) = factory.EnumAdapters1(i) {
        if a.EnumOutputs(0).is_ok() {
            return Ok(a);
        }
        i += 1;
    }
    Err(HwError::Unsupported("no adapter drives a screen".into()))
}
