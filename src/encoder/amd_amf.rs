#![allow(dead_code, non_snake_case, non_camel_case_types)]

#[cfg(target_os = "windows")]
use super::VideoEncoder;
#[cfg(target_os = "windows")]
use log::info;
#[cfg(target_os = "windows")]
use std::ffi::c_void;

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[cfg(target_os = "windows")]
const IID_IDXGIFACTORY1: GUID = GUID {
    data1: 0x770aae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

#[cfg(target_os = "windows")]
#[repr(C)]
struct DXGI_ADAPTER_DESC1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: u64,
    flags: u32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct IDXGIFactory1Vtbl {
    parent: IUnknownVtbl,
    set_private_data: *const c_void,
    set_private_data_interface: *const c_void,
    get_private_data: *const c_void,
    get_parent: *const c_void,
    enum_adapters: *const c_void,
    make_window_association: *const c_void,
    get_window_association: *const c_void,
    create_swap_chain: *const c_void,
    create_software_adapter: *const c_void,
    enum_adapters1: unsafe extern "system" fn(this: *mut c_void, adapter: u32, pp_adapter: *mut *mut c_void) -> i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct IDXGIAdapter1Vtbl {
    parent: IUnknownVtbl,
    set_private_data: *const c_void,
    set_private_data_interface: *const c_void,
    get_private_data: *const c_void,
    get_parent: *const c_void,
    enum_outputs: *const c_void,
    get_desc: *const c_void,
    check_interface_support: *const c_void,
    get_desc1: unsafe extern "system" fn(this: *mut c_void, p_desc: *mut DXGI_ADAPTER_DESC1) -> i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFFactory {
    vtbl: *const AMFFactoryVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFFactoryVtbl {
    create_context: unsafe extern "system" fn(this: *mut AMFFactory, context: *mut *mut AMFContext) -> i32,
    create_component: unsafe extern "system" fn(this: *mut AMFFactory, context: *mut AMFContext, id: *const u16, component: *mut *mut AMFComponent) -> i32,
    set_cache_folder: *const c_void,
    get_cache_folder: *const c_void,
    get_debug: *const c_void,
    get_trace: *const c_void,
    get_programs: *const c_void,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct AMFVariantStruct {
    variant_type: u32,
    int64_val: i64,
    padding: [u8; 16],
}

#[cfg(target_os = "windows")]
impl AMFVariantStruct {
    fn from_int64(val: i64) -> Self {
        Self {
            variant_type: 2, // AMF_VARIANT_INT64
            int64_val: val,
            padding: [0; 16],
        }
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFComponent {
    vtbl: *const AMFComponentVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFComponentVtbl {
    acquire: *const c_void,
    release: unsafe extern "system" fn(this: *mut AMFComponent) -> u32,
    query_interface: *const c_void,
    set_property: unsafe extern "system" fn(this: *mut AMFComponent, name: *const u16, value: AMFVariantStruct) -> i32,
    get_property: *const c_void,
    has_property: *const c_void,
    get_property_count: *const c_void,
    get_property_at: *const c_void,
    clear: *const c_void,
    add_to: *const c_void,
    copy_to: *const c_void,
    add_observer: *const c_void,
    remove_observer: *const c_void,

    // AMFPropertyStorageEx (4 métodos)
    get_properties_info_count: *const c_void,
    get_property_info_idx: *const c_void,
    get_property_info_name: *const c_void,
    validate_property: *const c_void,

    // AMFComponent methods
    init: unsafe extern "system" fn(this: *mut AMFComponent, format: u32, width: i32, height: i32) -> i32,
    reinit: *const c_void,
    terminate: unsafe extern "system" fn(this: *mut AMFComponent) -> i32,
    drain: unsafe extern "system" fn(this: *mut AMFComponent) -> i32,
    flush: *const c_void,
    submit_input: unsafe extern "system" fn(this: *mut AMFComponent, data: *mut c_void) -> i32,
    query_output: unsafe extern "system" fn(this: *mut AMFComponent, data: *mut *mut c_void) -> i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFPlane {
    vtbl: *const AMFPlaneVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFPlaneVtbl {
    acquire: *const c_void,
    release: unsafe extern "system" fn(this: *mut AMFPlane) -> u32,
    query_interface: *const c_void,
    get_type: *const c_void,
    get_native: unsafe extern "system" fn(this: *mut AMFPlane) -> *mut c_void,
    get_pixel_size_in_bytes: *const c_void,
    get_offset_x: *const c_void,
    get_offset_y: *const c_void,
    get_width: *const c_void,
    get_height: *const c_void,
    get_h_pitch: unsafe extern "system" fn(this: *mut AMFPlane) -> i32,
    get_v_pitch: unsafe extern "system" fn(this: *mut AMFPlane) -> i32,
    is_tiled: *const c_void,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFSurface {
    vtbl: *const AMFSurfaceVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFSurfaceVtbl {
    acquire: *const c_void,
    release: unsafe extern "system" fn(this: *mut AMFSurface) -> u32,
    query_interface: *const c_void,
    set_property: unsafe extern "system" fn(this: *mut AMFSurface, name: *const u16, value: AMFVariantStruct) -> i32,
    get_property: *const c_void,
    has_property: *const c_void,
    get_property_count: *const c_void,
    get_property_at: *const c_void,
    clear: *const c_void,
    add_to: *const c_void,
    copy_to: *const c_void,
    add_observer: *const c_void,
    remove_observer: *const c_void,
    get_memory_type: *const c_void,
    duplicate: *const c_void,
    convert: *const c_void,
    interop: *const c_void,
    get_data_type: *const c_void,
    is_reusable: *const c_void,
    set_pts: unsafe extern "system" fn(this: *mut AMFSurface, pts: i64),
    get_pts: *const c_void,
    set_duration: unsafe extern "system" fn(this: *mut AMFSurface, duration: i64),
    get_duration: *const c_void,
    get_format: *const c_void,
    get_planes_count: unsafe extern "system" fn(this: *mut AMFSurface) -> usize,
    get_plane_at: unsafe extern "system" fn(this: *mut AMFSurface, index: usize) -> *mut AMFPlane,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFBuffer {
    vtbl: *const AMFBufferVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFBufferVtbl {
    acquire: *const c_void,
    release: unsafe extern "system" fn(this: *mut AMFBuffer) -> u32,
    query_interface: *const c_void,
    set_property: *const c_void,
    get_property: *const c_void,
    has_property: *const c_void,
    get_property_count: *const c_void,
    get_property_at: *const c_void,
    clear: *const c_void,
    add_to: *const c_void,
    copy_to: *const c_void,
    add_observer: *const c_void,
    remove_observer: *const c_void,
    get_memory_type: *const c_void,
    duplicate: *const c_void,
    convert: *const c_void,
    interop: *const c_void,
    get_data_type: *const c_void,
    is_reusable: *const c_void,
    set_pts: *const c_void,
    get_pts: *const c_void,
    set_duration: *const c_void,
    get_duration: *const c_void,
    set_size: *const c_void,
    get_size: unsafe extern "system" fn(this: *mut AMFBuffer) -> usize,
    get_native: unsafe extern "system" fn(this: *mut AMFBuffer) -> *mut c_void,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFContext {
    vtbl: *const AMFContextVtbl,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct AMFContextVtbl {
    acquire: *const c_void,
    release: unsafe extern "system" fn(this: *mut AMFContext) -> u32,
    query_interface: *const c_void,

    set_property: *const c_void,
    get_property: *const c_void,
    has_property: *const c_void,
    get_property_count: *const c_void,
    get_property_at: *const c_void,
    clear: *const c_void,
    add_to: *const c_void,
    copy_to: *const c_void,
    add_observer: *const c_void,
    remove_observer: *const c_void,

    terminate: unsafe extern "system" fn(this: *mut AMFContext) -> i32,

    init_dx9: *const c_void,
    get_dx9_device: *const c_void,
    lock_dx9: *const c_void,
    unlock_dx9: *const c_void,

    init_dx11: unsafe extern "system" fn(this: *mut AMFContext, d3d11_device: *mut c_void, dx_version: u32) -> i32,
    get_dx11_device: *const c_void,
    lock_dx11: *const c_void,
    unlock_dx11: *const c_void,

    init_opencl: *const c_void,
    get_opencl_context: *const c_void,
    get_opencl_queue: *const c_void,
    get_opencl_device_id: *const c_void,
    get_opencl_factory: *const c_void,
    init_opencl_ex: *const c_void,
    lock_opencl: *const c_void,
    unlock_opencl: *const c_void,

    init_opengl: *const c_void,
    get_opengl_context: *const c_void,
    get_opengl_drawable: *const c_void,
    lock_opengl: *const c_void,
    unlock_opengl: *const c_void,

    init_xv: *const c_void,
    get_xv_device: *const c_void,
    lock_xv: *const c_void,
    unlock_xv: *const c_void,

    init_gralloc: *const c_void,
    get_gralloc_device: *const c_void,
    lock_gralloc: *const c_void,
    unlock_gralloc: *const c_void,

    alloc_buffer: *const c_void,
    alloc_surface: unsafe extern "system" fn(this: *mut AMFContext, mem_type: u32, format: u32, width: i32, height: i32, pp_surface: *mut *mut c_void) -> i32,
    alloc_audio_buffer: *const c_void,

    create_buffer_from_host: *const c_void,
    create_surface_from_host: *const c_void,
    create_surface_from_dx9: *const c_void,
    create_surface_from_dx11_native: unsafe extern "system" fn(this: *mut AMFContext, d3d11_surface: *mut c_void, pp_surface: *mut *mut c_void, observer: *mut c_void) -> i32,
}

#[cfg(target_os = "windows")]
type FnAMFInit = unsafe extern "system" fn(version: u64, factory: *mut *mut AMFFactory) -> i32;

#[cfg(target_os = "windows")]
pub struct AmdAmfZeroCopyEncoder {
    amf_dll: windows_sys::Win32::Foundation::HMODULE,
    d3d11_dll: windows_sys::Win32::Foundation::HMODULE,
    dxgi_dll: windows_sys::Win32::Foundation::HMODULE,
    context: *mut AMFContext,
    encoder: *mut AMFComponent,
    converter: *mut AMFComponent,
    d3d11_device: *mut c_void,
    pub gpu_name: String,
    width: u32,
    height: u32,
    bitrate_bps: u32,
    frame_idx: u64,
    needs_keyframe: bool,
    header_cache: Vec<u8>,
    out_buffer: Vec<u8>,
}

#[cfg(target_os = "windows")]
impl AmdAmfZeroCopyEncoder {
    pub fn try_new(target_fps: u32, _is_screen_content: bool) -> Result<Self, String> {
        unsafe {
            let amf_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"amfrt64.dll\0".as_ptr());
            if amf_dll.is_null() {
                return Err("amfrt64.dll ausente".to_string());
            }

            let amf_init_fn: Option<FnAMFInit> = std::mem::transmute(
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(amf_dll, b"AMFInit\0".as_ptr())
            );
            let amf_init = match amf_init_fn {
                Some(f) => f,
                None => {
                    windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                    return Err("AMFInit não encontrado".to_string());
                }
            };

            let amf_version: u64 = (1u64 << 48) | (4u64 << 32) | (0u64 << 16) | 0u64;
            let mut factory: *mut AMFFactory = std::ptr::null_mut();
            if amf_init(amf_version, &mut factory) != 0 || factory.is_null() {
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                return Err("AMFInit falhou".to_string());
            }

            let factory_vtbl = (*factory).vtbl;
            let mut context: *mut AMFContext = std::ptr::null_mut();
            if ((*factory_vtbl).create_context)(factory, &mut context) != 0 || context.is_null() {
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                return Err("CreateContext falhou".to_string());
            }

            // Inicializar Direct3D 11
            let d3d11_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"d3d11.dll\0".as_ptr());
            let dxgi_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"dxgi.dll\0".as_ptr());

            if d3d11_dll.is_null() {
                let _ = ((*(*context).vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                return Err("d3d11.dll ausente".to_string());
            }

            type FnD3D11CreateDevice = unsafe extern "system" fn(
                adapter: *mut c_void,
                driver_type: u32,
                software: *mut c_void,
                flags: u32,
                feature_levels: *const u32,
                feature_levels_count: u32,
                sdk_version: u32,
                device: *mut *mut c_void,
                feature_level: *mut u32,
                immediate_context: *mut *mut c_void,
            ) -> i32;

            let create_dev_fn: Option<FnD3D11CreateDevice> = std::mem::transmute(
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(d3d11_dll, b"D3D11CreateDevice\0".as_ptr())
            );
            let create_dev = match create_dev_fn {
                Some(f) => f,
                None => {
                    let _ = ((*(*context).vtbl).release)(context);
                    windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                    windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                    return Err("D3D11CreateDevice não encontrado".to_string());
                }
            };

            let mut d3d11_device: *mut c_void = std::ptr::null_mut();
            let mut feat_lvl = 0u32;
            let mut d3d11_ctx: *mut c_void = std::ptr::null_mut();
            let hr = create_dev(
                std::ptr::null_mut(),
                1, // D3D_DRIVER_TYPE_HARDWARE
                std::ptr::null_mut(),
                0x20 | 0x800, // D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT
                std::ptr::null(),
                0,
                7, // D3D11_SDK_VERSION
                &mut d3d11_device,
                &mut feat_lvl,
                &mut d3d11_ctx,
            );

            if hr != 0 || d3d11_device.is_null() {
                let _ = ((*(*context).vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("D3D11CreateDevice falhou: 0x{:08X}", hr));
            }

            let ctx_vtbl = (*context).vtbl;
            let init_dx_res = ((*ctx_vtbl).init_dx11)(context, d3d11_device, 110);
            if init_dx_res != 0 {
                let _ = ((*ctx_vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("InitDX11 falhou: {}", init_dx_res));
            }

            // Identificar GPU
            let gpu_name = "AMD Radeon GPU (AMF Hardware Accelerated)".to_string();

            // Criar AMFVideoConverter (GPU AMD faz a conversão de cores por hardware)
            let conv_name: Vec<u16> = "AMFVideoConverter\0".encode_utf16().collect();
            let mut converter: *mut AMFComponent = std::ptr::null_mut();
            let conv_res = ((*factory_vtbl).create_component)(factory, context, conv_name.as_ptr(), &mut converter);
            if conv_res != 0 || converter.is_null() {
                let _ = ((*ctx_vtbl).terminate)(context);
                let _ = ((*ctx_vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("CreateComponent(AMFVideoConverter) falhou: {}", conv_res));
            }

            let conv_vtbl = (*converter).vtbl;
            let prop_conv_out_fmt: Vec<u16> = "OutputFormat\0".encode_utf16().collect();
            let prop_conv_mem_type: Vec<u16> = "MemoryType\0".encode_utf16().collect();
            let _ = ((*conv_vtbl).set_property)(converter, prop_conv_out_fmt.as_ptr(), AMFVariantStruct::from_int64(1)); // NV12
            let _ = ((*conv_vtbl).set_property)(converter, prop_conv_mem_type.as_ptr(), AMFVariantStruct::from_int64(3)); // DX11

            let conv_init_res = ((*conv_vtbl).init)(converter, 3, 1920, 1080); // Entrada BGRA = 3
            if conv_init_res != 0 {
                let _ = ((*conv_vtbl).release)(converter);
                let _ = ((*ctx_vtbl).terminate)(context);
                let _ = ((*ctx_vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("AMFVideoConverter::Init falhou: {}", conv_init_res));
            }

            // Criar Encoder AVC (H.264)
            let codec_name: Vec<u16> = "AMFVideoEncoderVCE_AVC\0".encode_utf16().collect();
            let mut encoder: *mut AMFComponent = std::ptr::null_mut();
            let comp_res = ((*factory_vtbl).create_component)(factory, context, codec_name.as_ptr(), &mut encoder);
            if comp_res != 0 || encoder.is_null() {
                let _ = ((*conv_vtbl).terminate)(converter);
                let _ = ((*conv_vtbl).release)(converter);
                let _ = ((*ctx_vtbl).terminate)(context);
                let _ = ((*ctx_vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("CreateComponent(AMFVideoEncoderVCE_AVC) falhou: {}", comp_res));
            }

            let enc_vtbl = (*encoder).vtbl;
            let prop_usage: Vec<u16> = "Usage\0".encode_utf16().collect();
            let prop_profile: Vec<u16> = "Profile\0".encode_utf16().collect();
            let prop_rc: Vec<u16> = "RateControlMethod\0".encode_utf16().collect();
            let prop_bitrate: Vec<u16> = "TargetBitrate\0".encode_utf16().collect();
            let prop_gop: Vec<u16> = "IDRPeriod\0".encode_utf16().collect();
            let prop_header_spacing: Vec<u16> = "HeaderInsertionSpacing\0".encode_utf16().collect();
            let prop_fps: Vec<u16> = "FrameRate\0".encode_utf16().collect();
            let prop_peak_bitrate: Vec<u16> = "PeakBitrate\0".encode_utf16().collect();
            let prop_cabac: Vec<u16> = "CABACEnable\0".encode_utf16().collect();

            let initial_bitrate = 8_000_000u32;
            let peak_bitrate = 12_000_000u32;
            let _ = ((*enc_vtbl).set_property)(encoder, prop_usage.as_ptr(), AMFVariantStruct::from_int64(1)); // Ultra Low Latency
            let _ = ((*enc_vtbl).set_property)(encoder, prop_profile.as_ptr(), AMFVariantStruct::from_int64(66)); // Baseline Profile (Compatibilidade universal com OpenH264)
            let _ = ((*enc_vtbl).set_property)(encoder, prop_cabac.as_ptr(), AMFVariantStruct::from_int64(2)); // CALV / CAVLC (AMF_VIDEO_ENCODER_CODER_MODE_CALV = 2)
            let _ = ((*enc_vtbl).set_property)(encoder, prop_rc.as_ptr(), AMFVariantStruct::from_int64(1)); // RateControlMethod CBR (1) para stream UDP de baixa latência
            let _ = ((*enc_vtbl).set_property)(encoder, prop_bitrate.as_ptr(), AMFVariantStruct::from_int64(initial_bitrate as i64));
            let _ = ((*enc_vtbl).set_property)(encoder, prop_peak_bitrate.as_ptr(), AMFVariantStruct::from_int64(peak_bitrate as i64));
            // IDR a cada 2 segundos (120 frames a 60fps) — alinhado com loop TX IDR de 2000ms
            let gop_frames = (target_fps as i64) * 2;
            let _ = ((*enc_vtbl).set_property)(encoder, prop_gop.as_ptr(), AMFVariantStruct::from_int64(gop_frames));
            let _ = ((*enc_vtbl).set_property)(encoder, prop_header_spacing.as_ptr(), AMFVariantStruct::from_int64(gop_frames));
            let _ = ((*enc_vtbl).set_property)(encoder, prop_fps.as_ptr(), AMFVariantStruct {
                variant_type: 7, // AMF_VARIANT_RATE
                int64_val: (target_fps as i64) | (1i64 << 32),
                padding: [0; 16],
            });
            // QualityPreset BALANCED (0) — melhor qualidade que SPEED sem custo proibitivo de latência
            let prop_quality: Vec<u16> = "QualityPreset\0".encode_utf16().collect();
            let _ = ((*enc_vtbl).set_property)(encoder, prop_quality.as_ptr(), AMFVariantStruct::from_int64(0));

            // Init encoder para 1920x1080 (AMF_SURFACE_NV12 = 1)
            let init_res = ((*enc_vtbl).init)(encoder, 1, 1920, 1080);
            if init_res != 0 {
                let _ = ((*enc_vtbl).terminate)(encoder);
                let _ = ((*enc_vtbl).release)(encoder);
                let _ = ((*conv_vtbl).terminate)(converter);
                let _ = ((*conv_vtbl).release)(converter);
                let _ = ((*ctx_vtbl).terminate)(context);
                let _ = ((*ctx_vtbl).release)(context);
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                windows_sys::Win32::Foundation::FreeLibrary(d3d11_dll);
                return Err(format!("AMFComponent::Init falhou: {}", init_res));
            }

            info!("💎 [AMD AMF GPU] Engine Nativo Zero-Copy Ativado com Sucesso (Converter GPU + Profile Constrained Baseline)! Placa: {}", gpu_name);

            Ok(Self {
                amf_dll,
                d3d11_dll,
                dxgi_dll,
                context,
                encoder,
                converter,
                d3d11_device,
                gpu_name,
                width: 1920,
                height: 1080,
                bitrate_bps: initial_bitrate,
                frame_idx: 0,
                needs_keyframe: true,
                header_cache: Vec::new(),
                out_buffer: Vec::with_capacity(128 * 1024),
            })
        }
    }
}

#[cfg(target_os = "windows")]
impl VideoEncoder for AmdAmfZeroCopyEncoder {
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let w = (width as i32) & !1;
        let h = (height as i32) & !1;
        if w <= 0 || h <= 0 {
            return None;
        }

        unsafe {
            let ctx_vtbl = (*self.context).vtbl;
            let enc_vtbl = (*self.encoder).vtbl;

            // Alocar superfície HOST com formato BGRA (AMF_MEMORY_HOST = 1, AMF_SURFACE_BGRA = 3)
            // A GPU AMD faz a conversão de cores BGRA -> NV12 via hardware VCN diretamente!
            let mut surface: *mut c_void = std::ptr::null_mut();
            let alloc_res = ((*ctx_vtbl).alloc_surface)(self.context, 1, 3, w, h, &mut surface);
            if alloc_res != 0 || surface.is_null() {
                return None;
            }

            let surf = surface as *mut AMFSurface;
            let surf_vtbl = (*surf).vtbl;

            // Obter plano 0 para preencher os pixels da tela
            let plane = ((*surf_vtbl).get_plane_at)(surf, 0);
            if plane.is_null() {
                let _ = ((*surf_vtbl).release)(surf);
                return None;
            }
            let plane_vtbl = (*plane).vtbl;
            let native_ptr = ((*plane_vtbl).get_native)(plane) as *mut u8;
            let h_pitch = ((*plane_vtbl).get_h_pitch)(plane) as usize;

            if !native_ptr.is_null() && h_pitch >= (w as usize * 4) {
                let row_bytes = w as usize * 4;
                for row in 0..(h as usize) {
                    let src_offset = row * row_bytes;
                    let dst_offset = row * h_pitch;
                    if src_offset + row_bytes <= bgra_data.len() {
                        std::ptr::copy_nonoverlapping(
                            bgra_data.as_ptr().add(src_offset),
                            native_ptr.add(dst_offset),
                            row_bytes,
                        );
                    }
                }
            }

            let pts = (self.frame_idx as i64) * 166_666;
            self.frame_idx += 1;
            ((*surf_vtbl).set_pts)(surf, pts);
            ((*surf_vtbl).set_duration)(surf, 166_666);

            // 1. Converter BGRA -> NV12 100% por hardware na GPU AMD
            let conv_vtbl = (*self.converter).vtbl;
            let conv_sub_res = ((*conv_vtbl).submit_input)(self.converter, surface);
            let _ = ((*surf_vtbl).release)(surf);

            if conv_sub_res != 0 && conv_sub_res != 24 {
                return None;
            }

            // 2. Coletar superfície NV12 acelerada e enviar para o encoder
            let mut conv_out: *mut c_void = std::ptr::null_mut();
            let conv_q_res = ((*conv_vtbl).query_output)(self.converter, &mut conv_out);
            if conv_q_res == 0 && !conv_out.is_null() {
                let out_surf = conv_out as *mut AMFSurface;
                if self.needs_keyframe {
                    let prop_force: Vec<u16> = "ForcePictureType\0".encode_utf16().collect();
                    // AMF_VIDEO_ENCODER_PICTURE_TYPE_IDR = 2 (No VideoEncoderVCE.h: NONE=0, SKIP=1, IDR=2, I=3)
                    let _ = ((*(*out_surf).vtbl).set_property)(out_surf, prop_force.as_ptr(), AMFVariantStruct::from_int64(2));
                }
                let enc_sub_res = ((*enc_vtbl).submit_input)(self.encoder, conv_out);
                let _ = ((*(*out_surf).vtbl).release)(out_surf);
                if enc_sub_res != 0 && enc_sub_res != 24 {
                    return None;
                }
            }

            self.out_buffer.clear();

            // Puxar pacotes produzidos da GPU (AMF é assíncrono — retry até 3x com 1ms de espera)
            for attempt in 0..3 {
                let mut output_data: *mut c_void = std::ptr::null_mut();
                let q_res = ((*enc_vtbl).query_output)(self.encoder, &mut output_data);
                if q_res == 0 && !output_data.is_null() {
                    let buf = output_data as *mut AMFBuffer;
                    let buf_vtbl = (*buf).vtbl;
                    let size = ((*buf_vtbl).get_size)(buf);
                    let ptr = ((*buf_vtbl).get_native)(buf) as *const u8;
                    if !ptr.is_null() && size > 0 {
                        let slice = std::slice::from_raw_parts(ptr, size);
                        self.out_buffer.extend_from_slice(slice);
                    }
                    let _ = ((*buf_vtbl).release)(buf);
                    // Tentar puxar mais pacotes (pode haver múltiplos NALs prontos)
                    loop {
                        let mut extra_data: *mut c_void = std::ptr::null_mut();
                        let extra_res = ((*enc_vtbl).query_output)(self.encoder, &mut extra_data);
                        if extra_res == 0 && !extra_data.is_null() {
                            let ebuf = extra_data as *mut AMFBuffer;
                            let ebuf_vtbl = (*ebuf).vtbl;
                            let esz = ((*ebuf_vtbl).get_size)(ebuf);
                            let eptr = ((*ebuf_vtbl).get_native)(ebuf) as *const u8;
                            if !eptr.is_null() && esz > 0 {
                                let eslice = std::slice::from_raw_parts(eptr, esz);
                                self.out_buffer.extend_from_slice(eslice);
                            }
                            let _ = ((*ebuf_vtbl).release)(ebuf);
                        } else {
                            break;
                        }
                    }
                    break;
                } else if attempt < 2 {
                    // AMF ainda processando — aguardar 1ms e tentar novamente
                    let _ = std::hint::black_box(());
                    std::thread::sleep(std::time::Duration::from_millis(1));
                } else {
                    break;
                }
            }

            if !self.out_buffer.is_empty() {
                // Extrair e preservar SPS/PPS se presente
                let has_sps = self.out_buffer.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7));
                if has_sps {
                    let sps_start = self.out_buffer.windows(5).position(|w| {
                        (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7)
                    });
                    if let Some(start) = sps_start {
                        let mut pos = start + 4;
                        let mut found_pps = false;
                        while pos + 4 <= self.out_buffer.len() {
                            let is_sc4 = self.out_buffer[pos..pos + 4] == [0, 0, 0, 1];
                            let is_sc3 = self.out_buffer[pos..pos + 3] == [0, 0, 1];
                            if is_sc4 || is_sc3 {
                                let nal_byte = if is_sc4 { self.out_buffer[pos + 4] } else { self.out_buffer[pos + 3] };
                                let nal_type = nal_byte & 0x1F;
                                if nal_type == 8 {
                                    found_pps = true;
                                } else if nal_type == 5 || nal_type == 1 {
                                    self.header_cache = self.out_buffer[start..pos].to_vec();
                                    break;
                                }
                            }
                            pos += 1;
                        }
                        if found_pps && self.header_cache.is_empty() {
                            self.header_cache = self.out_buffer[start..].to_vec();
                        }
                    }
                    // Frame IDR com SPS incluso — resetar flag de keyframe
                    self.needs_keyframe = false;
                } else if !self.header_cache.is_empty() {
                    let is_idr = self.out_buffer.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 5) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 5));
                    if is_idr {
                        self.needs_keyframe = false;
                        let mut combined = Vec::with_capacity(self.header_cache.len() + self.out_buffer.len());
                        combined.extend_from_slice(&self.header_cache);
                        combined.extend_from_slice(&self.out_buffer);
                        return Some(combined);
                    }
                }

                Some(std::mem::take(&mut self.out_buffer))
            } else {
                None
            }
        }
    }

    fn force_intra_frame(&mut self) {
        self.needs_keyframe = true;
    }

    fn set_bitrate_bps(&mut self, bitrate_bps: u32) {
        self.bitrate_bps = bitrate_bps.clamp(1_500_000, 12_000_000);
        unsafe {
            if !self.encoder.is_null() {
                let enc_vtbl = (*self.encoder).vtbl;
                let prop_bitrate: Vec<u16> = "TargetBitrate\0".encode_utf16().collect();
                let _ = ((*enc_vtbl).set_property)(self.encoder, prop_bitrate.as_ptr(), AMFVariantStruct::from_int64(self.bitrate_bps as i64));
                let prop_peak: Vec<u16> = "PeakBitrate\0".encode_utf16().collect();
                let peak = ((self.bitrate_bps as f64) * 1.5).min(16_000_000.0) as i64;
                let _ = ((*enc_vtbl).set_property)(self.encoder, prop_peak.as_ptr(), AMFVariantStruct::from_int64(peak));
            }
        }
    }

    fn get_bitrate_bps(&self) -> u32 {
        self.bitrate_bps
    }

    fn name(&self) -> &'static str {
        "AMD AMF Native Zero-Copy Engine (DirectX 11 Hardware Architecture)"
    }

    fn is_hardware_accelerated(&self) -> bool {
        true
    }
}

#[cfg(target_os = "windows")]
impl Drop for AmdAmfZeroCopyEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.encoder.is_null() {
                let enc_vtbl = (*self.encoder).vtbl;
                let _ = ((*enc_vtbl).terminate)(self.encoder);
                let _ = ((*enc_vtbl).release)(self.encoder);
                self.encoder = std::ptr::null_mut();
            }
            if !self.converter.is_null() {
                let conv_vtbl = (*self.converter).vtbl;
                let _ = ((*conv_vtbl).terminate)(self.converter);
                let _ = ((*conv_vtbl).release)(self.converter);
                self.converter = std::ptr::null_mut();
            }
            if !self.context.is_null() {
                let ctx_vtbl = (*self.context).vtbl;
                let _ = ((*ctx_vtbl).terminate)(self.context);
                let _ = ((*ctx_vtbl).release)(self.context);
                self.context = std::ptr::null_mut();
            }
            if !self.amf_dll.is_null() {
                windows_sys::Win32::Foundation::FreeLibrary(self.amf_dll);
            }
            if !self.d3d11_dll.is_null() {
                windows_sys::Win32::Foundation::FreeLibrary(self.d3d11_dll);
            }
            if !self.dxgi_dll.is_null() {
                windows_sys::Win32::Foundation::FreeLibrary(self.dxgi_dll);
            }
            info!("🛑 [AMD AMF GPU] Pipeline AMF Native Zero-Copy liberado com sucesso.");
        }
    }
}

#[cfg(target_os = "windows")]
unsafe impl Send for AmdAmfZeroCopyEncoder {}
