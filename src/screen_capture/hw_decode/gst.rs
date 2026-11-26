//! Hardware H.264 decoding on Linux through the system's GStreamer, when it is
//! installed: V4L2 stateless decoders on embedded boards (`v4l2slh264dec`,
//! STM32MP2, Rockchip, Raspberry Pi...), VA-API on Intel/AMD desktops, NVDEC on
//! NVIDIA. GStreamer is opened at run time (dlopen), so it is not a build or
//! install dependency; without it, or without a hardware decoder in it, streams
//! keep using OpenH264 on the CPU.
//!
//! `LITECORD_GST_DECODER=<element>` picks the decoder; `LITECORD_GST_DECODER=off`
//! turns this path off.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::OnceLock;

use slint::{Rgba8Pixel, SharedPixelBuffer};

use super::HwError;

/// Tried in this order when no decoder is named.
const DECODERS: &[&str] = &["v4l2slh264dec", "v4l2h264dec", "vah264dec", "vaapih264dec", "nvh264dec"];

const GST_STATE_NULL: c_int = 1;
const GST_STATE_PLAYING: c_int = 4;
const GST_MAP_READ: c_int = 1;
const GST_MESSAGE_ERROR: c_int = 1 << 1;
const GST_STATE_CHANGE_FAILURE: c_int = 0;

#[repr(C)]
struct GstMapInfo {
    memory: *mut c_void,
    flags: c_int,
    data: *mut u8,
    size: usize,
    maxsize: usize,
    user_data: [*mut c_void; 4],
    reserved: [*mut c_void; 4],
}

type P = *mut c_void;

struct Api {
    _core: libloading::Library,
    _app: libloading::Library,
    parse_launch: unsafe extern "C" fn(*const c_char, *mut P) -> P,
    bin_get_by_name: unsafe extern "C" fn(P, *const c_char) -> P,
    set_state: unsafe extern "C" fn(P, c_int) -> c_int,
    object_unref: unsafe extern "C" fn(P),
    mini_object_unref: unsafe extern "C" fn(P),
    get_bus: unsafe extern "C" fn(P) -> P,
    bus_pop_filtered: unsafe extern "C" fn(P, c_int) -> P,
    buffer_new_allocate: unsafe extern "C" fn(P, usize, P) -> P,
    buffer_fill: unsafe extern "C" fn(P, usize, *const c_void, usize) -> usize,
    buffer_map: unsafe extern "C" fn(P, *mut GstMapInfo, c_int) -> c_int,
    buffer_unmap: unsafe extern "C" fn(P, *mut GstMapInfo),
    sample_get_buffer: unsafe extern "C" fn(P) -> P,
    sample_get_caps: unsafe extern "C" fn(P) -> P,
    caps_get_structure: unsafe extern "C" fn(P, u32) -> P,
    structure_get_int: unsafe extern "C" fn(P, *const c_char, *mut c_int) -> c_int,
    push_buffer: unsafe extern "C" fn(P, P) -> c_int,
    try_pull_sample: unsafe extern "C" fn(P, u64) -> P,
    /// The decoder element to use.
    decoder: String,
}

// The function pointers and libraries are safe to share; GStreamer is thread-safe.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let wanted = std::env::var("LITECORD_GST_DECODER").unwrap_or_default();
        if wanted == "off" || wanted == "0" {
            return None;
        }
        let api = unsafe { load(wanted) };
        match &api {
            Some(a) => log::info!("🎞️ [GSTREAMER] Decodificador de hardware disponível: {}", a.decoder),
            None => log::info!("🎞️ [GSTREAMER] Sem GStreamer com decodificador H.264 de hardware; transmissões vão pela CPU"),
        }
        api
    })
    .as_ref()
}

unsafe fn load(wanted: String) -> Option<Api> {
    unsafe {
        let core = libloading::Library::new("libgstreamer-1.0.so.0").ok()?;
        let app = libloading::Library::new("libgstapp-1.0.so.0").ok()?;
        macro_rules! sym {
            ($lib:expr, $name:literal) => {
                *$lib.get(concat!($name, "\0").as_bytes()).ok()?
            };
        }
        let init: unsafe extern "C" fn(*mut c_int, *mut c_void) -> c_int = sym!(core, "gst_init_check");
        if init(std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
            return None;
        }
        let factory_find: unsafe extern "C" fn(*const c_char) -> P = sym!(core, "gst_element_factory_find");
        let object_unref: unsafe extern "C" fn(P) = sym!(core, "gst_object_unref");
        let has = |name: &str| {
            let Ok(c) = CString::new(name) else { return false };
            let f = factory_find(c.as_ptr());
            if f.is_null() {
                return false;
            }
            object_unref(f);
            true
        };
        let decoder = if wanted.is_empty() {
            DECODERS.iter().find(|d| has(d))?.to_string()
        } else if has(&wanted) {
            wanted
        } else {
            log::warn!("🎞️ [GSTREAMER] Decodificador '{}' não encontrado", wanted);
            return None;
        };
        // h264parse and videoconvert must exist too.
        if !has("h264parse") || !has("videoconvert") || !has("appsrc") {
            return None;
        }
        Some(Api {
            parse_launch: sym!(core, "gst_parse_launch"),
            bin_get_by_name: sym!(core, "gst_bin_get_by_name"),
            set_state: sym!(core, "gst_element_set_state"),
            object_unref,
            mini_object_unref: sym!(core, "gst_mini_object_unref"),
            get_bus: sym!(core, "gst_element_get_bus"),
            bus_pop_filtered: sym!(core, "gst_bus_pop_filtered"),
            buffer_new_allocate: sym!(core, "gst_buffer_new_allocate"),
            buffer_fill: sym!(core, "gst_buffer_fill"),
            buffer_map: sym!(core, "gst_buffer_map"),
            buffer_unmap: sym!(core, "gst_buffer_unmap"),
            sample_get_buffer: sym!(core, "gst_sample_get_buffer"),
            sample_get_caps: sym!(core, "gst_sample_get_caps"),
            caps_get_structure: sym!(core, "gst_caps_get_structure"),
            structure_get_int: sym!(core, "gst_structure_get_int"),
            push_buffer: sym!(app, "gst_app_src_push_buffer"),
            try_pull_sample: sym!(app, "gst_app_sink_try_pull_sample"),
            decoder,
            _core: core,
            _app: app,
        })
    }
}

/// One stream's pipeline: appsrc ! h264parse ! <hw decoder> ! videoconvert ! RGBA appsink.
pub struct GstDecoder {
    api: &'static Api,
    pipeline: P,
    src: P,
    sink: P,
    bus: P,
}

// Used from the one thread that decodes the stream.
unsafe impl Send for GstDecoder {}

impl GstDecoder {
    pub fn new() -> Result<Self, HwError> {
        let api = api().ok_or_else(|| HwError::Unsupported("no GStreamer hardware H.264 decoder".into()))?;
        let desc = format!(
            "appsrc name=src is-live=true do-timestamp=true format=time \
             caps=video/x-h264,stream-format=byte-stream,alignment=au \
             ! h264parse ! {} ! videoconvert n-threads=2 ! video/x-raw,format=RGBA \
             ! appsink name=sink sync=false max-buffers=2 drop=true",
            api.decoder
        );
        let desc = CString::new(desc).map_err(|e| HwError::Unsupported(e.to_string()))?;
        unsafe {
            let pipeline = (api.parse_launch)(desc.as_ptr(), std::ptr::null_mut());
            if pipeline.is_null() {
                return Err(HwError::Unsupported("gst_parse_launch failed".into()));
            }
            let src = (api.bin_get_by_name)(pipeline, c"src".as_ptr());
            let sink = (api.bin_get_by_name)(pipeline, c"sink".as_ptr());
            let bus = (api.get_bus)(pipeline);
            let dec = GstDecoder { api, pipeline, src, sink, bus };
            if src.is_null() || sink.is_null() || bus.is_null() {
                return Err(HwError::Unsupported("pipeline without appsrc/appsink".into()));
            }
            if (api.set_state)(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE {
                return Err(HwError::Unsupported("pipeline did not start".into()));
            }
            Ok(dec)
        }
    }

    /// Pushes one access unit and returns the newest decoded picture, if any.
    pub fn decode(&mut self, au: &[u8]) -> Result<Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)>, HwError> {
        let a = self.api;
        unsafe {
            let buf = (a.buffer_new_allocate)(std::ptr::null_mut(), au.len(), std::ptr::null_mut());
            if buf.is_null() {
                return Err(HwError::Device("buffer allocation failed".into()));
            }
            (a.buffer_fill)(buf, 0, au.as_ptr().cast(), au.len());
            // The buffer is owned by appsrc from here.
            if (a.push_buffer)(self.src, buf) != 0 {
                return Err(HwError::Device("appsrc refused the buffer".into()));
            }
            let msg = (a.bus_pop_filtered)(self.bus, GST_MESSAGE_ERROR);
            if !msg.is_null() {
                (a.mini_object_unref)(msg);
                return Err(HwError::Device("GStreamer pipeline error".into()));
            }
            // Keep only the newest picture; the first wait is short so a hardware
            // decoder with a frame of latency still shows each picture once.
            let mut newest = None;
            let mut timeout: u64 = 8_000_000; // 8 ms
            loop {
                let sample = (a.try_pull_sample)(self.sink, timeout);
                if sample.is_null() {
                    break;
                }
                timeout = 0;
                if let Some(p) = self.read_sample(sample) {
                    newest = Some(p);
                }
                (a.mini_object_unref)(sample);
            }
            Ok(newest)
        }
    }

    unsafe fn read_sample(&self, sample: P) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
        let a = self.api;
        unsafe {
            let caps = (a.sample_get_caps)(sample);
            if caps.is_null() {
                return None;
            }
            let s = (a.caps_get_structure)(caps, 0);
            let (mut w, mut h) = (0 as c_int, 0 as c_int);
            if (a.structure_get_int)(s, c"width".as_ptr(), &mut w) == 0 || (a.structure_get_int)(s, c"height".as_ptr(), &mut h) == 0 {
                return None;
            }
            let (w, h) = (w.max(0) as usize, h.max(0) as usize);
            let buffer = (a.sample_get_buffer)(sample);
            if buffer.is_null() || w == 0 || h == 0 {
                return None;
            }
            let mut map: GstMapInfo = std::mem::zeroed();
            if (a.buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                return None;
            }
            let data = std::slice::from_raw_parts(map.data, map.size);
            let row = w * 4;
            let stride = if h > 0 { (map.size / h).max(row) } else { row };
            let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
            let dst = out.make_mut_bytes();
            let ok = stride * (h - 1) + row <= data.len();
            if ok {
                for y in 0..h {
                    dst[y * row..(y + 1) * row].copy_from_slice(&data[y * stride..y * stride + row]);
                }
            }
            (a.buffer_unmap)(buffer, &mut map);
            ok.then_some((out, w as u32, h as u32))
        }
    }
}

impl Drop for GstDecoder {
    fn drop(&mut self) {
        let a = self.api;
        unsafe {
            if !self.pipeline.is_null() {
                (a.set_state)(self.pipeline, GST_STATE_NULL);
            }
            for p in [self.src, self.sink, self.bus, self.pipeline] {
                if !p.is_null() {
                    (a.object_unref)(p);
                }
            }
        }
    }
}
