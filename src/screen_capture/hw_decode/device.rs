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
        // The context holds what was bound to it and what waits to be destroyed, and with
        // them the device: without this the device outlived us with ~25 objects in it.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
        log::info!("🎞️ [GPU VIDEO] Dispositivo da GPU liberado (ninguém assiste pela GPU)");
        unsafe { report_live_objects(&self.device) };
    }
}

/// With `LITECORD_D3D_DEBUG` set (and the Windows "Graphics Tools" feature installed) the
/// device is created with the debug layer, and this logs the objects still alive when it
/// goes: only the device and its context should be left.
unsafe fn report_live_objects(device: &ID3D11Device) {
    let (Ok(dbg), Ok(q)) = (device.cast::<ID3D11Debug>(), device.cast::<ID3D11InfoQueue>()) else { return };
    q.ClearStoredMessages();
    let _ = dbg.ReportLiveDeviceObjects(D3D11_RLDO_DETAIL | D3D11_RLDO_IGNORE_INTERNAL);
    for i in 0..q.GetNumStoredMessages() {
        let mut len = 0usize;
        let _ = q.GetMessage(i, None, &mut len);
        let mut buf = vec![0u64; len / 8 + 1];
        let m = buf.as_mut_ptr() as *mut D3D11_MESSAGE;
        if q.GetMessage(i, Some(m), &mut len).is_ok() {
            let d = &*m;
            let text = std::slice::from_raw_parts(d.pDescription as *const u8, d.DescriptionByteLength.saturating_sub(1));
            log::info!("🎞️ [GPU VIDEO] D3D11 vivo: {}", String::from_utf8_lossy(text));
        }
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
    D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, None, D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT | if std::env::var("LITECORD_D3D_DEBUG").is_ok() { D3D11_CREATE_DEVICE_DEBUG } else { D3D11_CREATE_DEVICE_FLAG(0) }, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
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
