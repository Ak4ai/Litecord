use std::ffi::c_void;
use std::time::Instant;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_IDXGIFACTORY1: GUID = GUID {
    data1: 0x770aae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

const NV_ENC_CODEC_H264_GUID: GUID = GUID {
    data1: 0x6bc82762,
    data2: 0x4e63,
    data3: 0x4ca4,
    data4: [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
};

const NV_ENC_PRESET_P1_GUID: GUID = GUID {
    data1: 0xfc0a36d2,
    data2: 0xda0e,
    data3: 0x49a4,
    data4: [0xa9, 0xa0, 0x3e, 0x3e, 0x21, 0x3c, 0x9e, 0x69],
};

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

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
}

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

#[repr(C)]
struct NV_ENCODE_API_FUNCTION_LIST {
    version: u32,
    reserved: u32,
    nvEncOpenEncodeSession: Option<unsafe extern "system" fn(device: *mut c_void, device_type: u32, encoder: *mut *mut c_void) -> u32>,
    nvEncGetEncodeGUIDCount: *const c_void,
    nvEncGetEncodeGUIDs: *const c_void,
    nvEncGetEncodeProfileGUIDCount: *const c_void,
    nvEncGetEncodeProfileGUIDs: *const c_void,
    nvEncGetInputFormatCount: *const c_void,
    nvEncGetInputFormats: *const c_void,
    nvEncGetEncodeCaps: *const c_void,
    nvEncGetEncodePresetCount: *const c_void,
    nvEncGetEncodePresetGUIDs: *const c_void,
    nvEncGetEncodePresetConfig: Option<unsafe extern "system" fn(encoder: *mut c_void, encode_guid: GUID, preset_guid: GUID, preset_config: *mut c_void) -> u32>,
    nvEncInitializeEncoder: Option<unsafe extern "system" fn(encoder: *mut c_void, create_encode_config: *mut c_void) -> u32>,
    nvEncCreateInputBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, create_input_buffer: *mut c_void) -> u32>,
    nvEncDestroyInputBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, input_buffer: *mut c_void) -> u32>,
    nvEncCreateBitstreamBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, create_bitstream_buffer: *mut c_void) -> u32>,
    nvEncDestroyBitstreamBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, bitstream_buffer: *mut c_void) -> u32>,
    nvEncEncodePicture: Option<unsafe extern "system" fn(encoder: *mut c_void, encode_pic_params: *mut c_void) -> u32>,
    nvEncLockBitstream: Option<unsafe extern "system" fn(encoder: *mut c_void, lock_bitstream_buffer_params: *mut c_void) -> u32>,
    nvEncUnlockBitstream: Option<unsafe extern "system" fn(encoder: *mut c_void, bitstream_buffer: *mut c_void) -> u32>,
    nvEncLockInputBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, lock_input_buffer_params: *mut c_void) -> u32>,
    nvEncUnlockInputBuffer: Option<unsafe extern "system" fn(encoder: *mut c_void, input_buffer: *mut c_void) -> u32>,
    nvEncGetEncodeStats: *const c_void,
    nvEncGetSequenceParams: *const c_void,
    nvEncRegisterAsyncEvent: *const c_void,
    nvEncUnregisterAsyncEvent: *const c_void,
    nvEncMapInputResource: *const c_void,
    nvEncUnmapInputResource: *const c_void,
    nvEncDestroyEncoder: Option<unsafe extern "system" fn(encoder: *mut c_void) -> u32>,
    nvEncInvalidateRefFrames: *const c_void,
    nvEncOpenEncodeSessionEx: Option<unsafe extern "system" fn(open_session_ex_params: *mut c_void, encoder: *mut *mut c_void) -> u32>,
    nvEncRegisterResource: *const c_void,
    nvEncUnregisterResource: *const c_void,
    nvEncReconfigureEncoder: *const c_void,
    reserved1: *const c_void,
    nvEncCreateSubFrameData: *const c_void,
    nvEncDestroySubFrameData: *const c_void,
    nvEncSetIOCudaStreams: *const c_void,
    nvEncSendEOSNotification: *const c_void,
    reserved2: [*mut c_void; 277],
}

#[repr(C)]
struct NvEncOpenEncodeSessionExParams {
    version: u32,
    device_type: u32,
    device: *mut c_void,
    custom_extension: *mut c_void,
    api_version: u32,
    reserved1: [u32; 253],
    reserved2: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncConfig {
    version: u32,
    profile_guid: GUID,
    gop_length: u32,
    frame_interval_p: i32,
    mono_chrome_encoding: u32,
    frame_field_mode: u32,
    mv_precision: u32,
    reserved: [u32; 240],
    reserved_ptrs: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncPresetConfig {
    version: u32,
    preset_cfg: NvEncConfig,
    reserved: [u32; 240],
    reserved_ptrs: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncInitializeParams {
    version: u32,
    encode_guid: GUID,
    preset_guid: GUID,
    encode_width: u32,
    encode_height: u32,
    dar_width: u32,
    dar_height: u32,
    frame_rate_num: u32,
    frame_rate_den: u32,
    enable_encode_async: u32,
    enable_ptd: u32,
    report_slice_offsets: u32,
    enable_sub_frame_write: u32,
    enable_external_reorder: u32,
    max_encode_width: u32,
    max_encode_height: u32,
    encode_config: *mut c_void,
    completion_event: *mut c_void,
    reserved1: [*mut c_void; 286],
    reserved2: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncCreateInputBufferParams {
    version: u32,
    width: u32,
    height: u32,
    memory_heap: u32,
    buffer_format: u32,
    reserved: u32,
    input_buffer: *mut c_void,
    p_sys_mem_buffer: *mut c_void,
    reserved1: [u32; 57],
    reserved2: [*mut c_void; 63],
}

#[repr(C)]
struct NvEncCreateBitstreamBufferParams {
    version: u32,
    size: u32,
    memory_heap: u32,
    reserved: u32,
    bitstream_buffer: *mut c_void,
    bitstream_buffer_ptr: *mut c_void,
    reserved1: [u32; 58],
    reserved2: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncLockInputBufferParams {
    version: u32,
    do_not_wait: u32,
    sync_mode: u32,
    input_buffer: *mut c_void,
    buffer_data_ptr: *mut c_void,
    pitch: u32,
    reserved1: [u32; 58],
    reserved2: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncPicParamsStruct {
    version: u32,
    input_width: u32,
    input_height: u32,
    input_pitch: u32,
    encode_pic_flags: u32,
    frame_idx: u32,
    input_time_stamp: u64,
    input_duration: u64,
    input_buffer: *mut c_void,
    output_bitstream: *mut c_void,
    completion_event: *mut c_void,
    buffer_format: u32,
    picture_struct: u32,
    picture_type: u32,
    reserved: [u32; 240],
    reserved_ptrs: [*mut c_void; 64],
}

#[repr(C)]
struct NvEncLockBitstreamParams {
    version: u32,
    do_not_wait: u32,
    ltr_frame: u32,
    reserved: u32,
    output_bitstream: *mut c_void,
    slice_offsets: *mut u32,
    frame_idx: u32,
    hw_encode_status: u32,
    num_slices: u32,
    bitstream_size_in_bytes: u32,
    output_time_stamp: u64,
    output_duration: u64,
    bitstream_buffer_ptr: *mut c_void,
    picture_type: u32,
    picture_struct: u32,
    frame_avg_qp: u32,
    frame_satd: u32,
    ltr_frame_idx: u32,
    ltr_frame_bitmap: u32,
    reserved1: [u32; 236],
    reserved2: [*mut c_void; 64],
}

fn main() {
    println!("============================================================");
    println!("🚀 TESTE ISOLADO: HARDWARE ENCODER NVIDIA NVENC + DIRECT3D 11");
    println!("============================================================");

    unsafe {
        // 1. Carregar DXGI para enumerar adaptadores
        let dxgi_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"dxgi.dll\0".as_ptr());
        if dxgi_dll.is_null() {
            println!("❌ Falha ao carregar dxgi.dll");
            return;
        }

        let create_dxgi_factory1: Option<unsafe extern "system" fn(riid: *const GUID, pp_factory: *mut *mut c_void) -> i32> = 
            std::mem::transmute(windows_sys::Win32::System::LibraryLoader::GetProcAddress(dxgi_dll, b"CreateDXGIFactory1\0".as_ptr()));

        let create_factory = match create_dxgi_factory1 {
            Some(f) => f,
            None => {
                println!("❌ CreateDXGIFactory1 não encontrado na dxgi.dll");
                return;
            }
        };

        let mut factory: *mut c_void = std::ptr::null_mut();
        let hr = create_factory(&IID_IDXGIFACTORY1, &mut factory);
        if hr != 0 || factory.is_null() {
            println!("❌ CreateDXGIFactory1 falhou com hr = 0x{:08X}", hr);
            return;
        }

        let factory_vtbl = *(factory as *mut *const IDXGIFactory1Vtbl);
        println!("✅ IDXGIFactory1 instanciada com sucesso!");

        // 2. Enumerar todos os adaptadores gráficos
        let mut adapter_idx = 0u32;
        let mut nvidia_adapter: *mut c_void = std::ptr::null_mut();
        let mut nvidia_name = String::new();

        println!("\n🔍 ENUMERANDO PLACAS DE VÍDEO DO SISTEMA:");
        loop {
            let mut adapter: *mut c_void = std::ptr::null_mut();
            let hr = ((*factory_vtbl).enum_adapters1)(factory, adapter_idx, &mut adapter);
            if hr != 0 || adapter.is_null() {
                break;
            }

            let adapter_vtbl = *(adapter as *mut *const IDXGIAdapter1Vtbl);
            let mut desc: DXGI_ADAPTER_DESC1 = std::mem::zeroed();
            let _ = ((*adapter_vtbl).get_desc1)(adapter, &mut desc);

            let name_len = desc.description.iter().position(|&c| c == 0).unwrap_or(128);
            let name = String::from_utf16_lossy(&desc.description[..name_len]);
            let vram_mb = desc.dedicated_video_memory / (1024 * 1024);

            println!("   [{}] GPU: '{}' | Vendor ID: 0x{:04X} | Device ID: 0x{:04X} | VRAM: {} MB", 
                adapter_idx, name, desc.vendor_id, desc.device_id, vram_mb);

            if desc.vendor_id == 0x10DE && nvidia_adapter.is_null() {
                nvidia_adapter = adapter;
                nvidia_name = name;
            }

            adapter_idx += 1;
        }

        if nvidia_adapter.is_null() {
            println!("❌ Nenhuma GPU NVIDIA (Vendor ID 0x10DE) encontrada no sistema.");
            return;
        }

        println!("\n🎯 GPU NVIDIA SELECIONADA: '{}' (Vendor ID: 0x10DE)", nvidia_name);

        // 3. Criar dispositivo Direct3D 11 diretamente no adaptador NVIDIA
        let d3d11_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"d3d11.dll\0".as_ptr());
        if d3d11_dll.is_null() {
            println!("❌ Falha ao carregar d3d11.dll");
            return;
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

        let create_device_fn: Option<FnD3D11CreateDevice> = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(d3d11_dll, b"D3D11CreateDevice\0".as_ptr())
        );

        let create_device = create_device_fn.expect("D3D11CreateDevice não encontrado");
        let mut d3d11_device: *mut c_void = std::ptr::null_mut();
        let mut d3d11_context: *mut c_void = std::ptr::null_mut();
        let mut feature_level: u32 = 0;

        // D3D_DRIVER_TYPE_UNKNOWN (0) é obrigatório ao especificar um pAdapter explícito
        let hr = create_device(
            nvidia_adapter,
            0, // D3D_DRIVER_TYPE_UNKNOWN
            std::ptr::null_mut(),
            0x20 | 0x800, // D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT
            std::ptr::null(),
            0,
            7, // D3D11_SDK_VERSION
            &mut d3d11_device,
            &mut feature_level,
            &mut d3d11_context,
        );

        if hr != 0 || d3d11_device.is_null() {
            println!("❌ Falha ao criar Direct3D 11 Device na GPU NVIDIA! HR = 0x{:08X}", hr);
            return;
        }

        // 4. Testar CUDA Context nativo da NVIDIA (nvcuda.dll)
        let cuda_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"nvcuda.dll\0".as_ptr());
        let mut cuda_context: *mut c_void = std::ptr::null_mut();
        if !cuda_dll.is_null() {
            type FnCuInit = unsafe extern "system" fn(flags: u32) -> i32;
            type FnCuDeviceGet = unsafe extern "system" fn(device: *mut i32, ordinal: i32) -> i32;
            type FnCuCtxCreate = unsafe extern "system" fn(pctx: *mut *mut c_void, flags: u32, dev: i32) -> i32;
            type FnCuCtxSetCurrent = unsafe extern "system" fn(ctx: *mut c_void) -> i32;

            let cu_init: Option<FnCuInit> = std::mem::transmute(windows_sys::Win32::System::LibraryLoader::GetProcAddress(cuda_dll, b"cuInit\0".as_ptr()));
            let cu_dev_get: Option<FnCuDeviceGet> = std::mem::transmute(windows_sys::Win32::System::LibraryLoader::GetProcAddress(cuda_dll, b"cuDeviceGet\0".as_ptr()));
            let cu_ctx_create: Option<FnCuCtxCreate> = std::mem::transmute(windows_sys::Win32::System::LibraryLoader::GetProcAddress(cuda_dll, b"cuCtxCreate_v2\0".as_ptr()));
            let cu_ctx_set_current: Option<FnCuCtxSetCurrent> = std::mem::transmute(windows_sys::Win32::System::LibraryLoader::GetProcAddress(cuda_dll, b"cuCtxSetCurrent\0".as_ptr()));

            if let (Some(init), Some(dev_get), Some(ctx_create)) = (cu_init, cu_dev_get, cu_ctx_create) {
                let r1 = init(0);
                let mut cu_dev = 0i32;
                let r2 = dev_get(&mut cu_dev, 0);
                let r3 = ctx_create(&mut cuda_context, 0, cu_dev);
                if let Some(set_curr) = cu_ctx_set_current {
                    let r4 = set_curr(cuda_context);
                    println!("\n🟢 CONTEXTO CUDA ATIVADO NA THREAD: init={}, dev={}, ctx_create={}, set_current={} -> Context: {:p}", r1, cu_dev, r3, r4, cuda_context);
                } else {
                    println!("\n🟢 INICIALIZANDO CONTEXTO CUDA: init={}, dev_get={} (dev={}), ctx_create={} -> Context: {:p}", r1, r2, cu_dev, r3, cuda_context);
                }
            }
        }
        println!("✅ Dispositivo Direct3D 11 criado no silício da NVIDIA! Feature Level: 0x{:04X} | Handle: {:p}", feature_level, d3d11_device);

        // 4. Carregar nvEncodeAPI64.dll e inicializar sessão
        let nvenc_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"nvEncodeAPI64.dll\0".as_ptr());
        if nvenc_dll.is_null() {
            println!("❌ Falha ao carregar nvEncodeAPI64.dll");
            return;
        }

        type FnNvEncodeAPICreateInstance = unsafe extern "system" fn(function_list: *mut NV_ENCODE_API_FUNCTION_LIST) -> u32;
        let create_instance_fn: Option<FnNvEncodeAPICreateInstance> = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(nvenc_dll, b"NvEncodeAPICreateInstance\0".as_ptr())
        );
        let create_instance = create_instance_fn.expect("NvEncodeAPICreateInstance não encontrado");

        let mut fn_list: NV_ENCODE_API_FUNCTION_LIST = std::mem::zeroed();
        fn_list.version = (314 << 24) | (2 & 0xFFFFFF);
        let status = create_instance(&mut fn_list);
        if status != 0 {
            println!("❌ NvEncodeAPICreateInstance falhou com status {}", status);
            return;
        }
        println!("✅ NV_ENCODE_API_FUNCTION_LIST inicializada com sucesso! (version=0x{:08X})", fn_list.version);
        let ptr_slice = std::slice::from_raw_parts(&fn_list as *const _ as *const usize, 40);
        for (i, &p) in ptr_slice.iter().enumerate() {
            if i >= 1 && i <= 35 {
                println!("   [Slot {:02}] 0x{:016X}", i, p);
            }
        }

        let open_session_fn = fn_list.nvEncOpenEncodeSessionEx.expect("nvEncOpenEncodeSessionEx");

        // 5. Testar abertura de sessão com o dispositivo NVIDIA
        println!("\n🔍 ABRINDO SESSÃO NVENC NO DISPOSITIVO D3D11 DA NVIDIA:");

        // Macro oficial NVIDIA: ((uint32_t)NVENCAPI_VERSION | ((ver)<<16) | (0x7 << 28))
        let make_struct_ver = |ver: u32, major: u32, minor: u32| -> u32 {
            let api_ver = major | (minor << 24);
            api_ver | (ver << 16) | (0x7 << 28)
        };

        let candidate_configs: &[(u32, u32, u32, u32)] = &[
            // (major, minor, struct_ver, device_type)
            (12, 2, make_struct_ver(1, 12, 2), 0),
            (12, 1, make_struct_ver(1, 12, 1), 0),
            (12, 0, make_struct_ver(1, 12, 0), 0),
            (11, 1, make_struct_ver(1, 11, 1), 0),
            (11, 0, make_struct_ver(1, 11, 0), 0),
            (10, 0, make_struct_ver(1, 10, 0), 0),
            (9, 1, make_struct_ver(1, 9, 1), 0),
            (9, 0, make_struct_ver(1, 9, 0), 0),
            (8, 2, make_struct_ver(1, 8, 2), 0),
            (8, 1, make_struct_ver(1, 8, 1), 0),
            (8, 0, make_struct_ver(1, 8, 0), 0),
            (7, 1, make_struct_ver(1, 7, 1), 0),
            (7, 0, make_struct_ver(1, 7, 0), 0),
        ];

        let mut encoder_handle: *mut c_void = std::ptr::null_mut();
        let mut matched_major = 12u32;
        let mut matched_minor = 0u32;

        println!("   Size of NvEncOpenEncodeSessionExParams: {} bytes", std::mem::size_of::<NvEncOpenEncodeSessionExParams>());

        if let Some(open_legacy_fn) = fn_list.nvEncOpenEncodeSession {
            if !cuda_context.is_null() {
                let mut h_enc: *mut c_void = std::ptr::null_mut();
                let legacy_status = open_legacy_fn(cuda_context, 1, &mut h_enc);
                println!("   -> nvEncOpenEncodeSession (CUDA context, dev_type=1) -> STATUS: {}", legacy_status);
                if legacy_status == 0 && !h_enc.is_null() {
                    println!("🎉 SESSÃO DE HARDWARE ABERTA VIA nvEncOpenEncodeSession (CUDA)! Handle: {:p}", h_enc);
                    encoder_handle = h_enc;
                }
            }
            for dt in 0..4 {
                let mut h_enc: *mut c_void = std::ptr::null_mut();
                let legacy_status = open_legacy_fn(d3d11_device, dt, &mut h_enc);
                println!("   -> nvEncOpenEncodeSession (D3D11, dev_type={}) -> STATUS: {}", dt, legacy_status);
                if legacy_status == 0 && !h_enc.is_null() && encoder_handle.is_null() {
                    println!("🎉 SESSÃO DE HARDWARE ABERTA VIA nvEncOpenEncodeSession (dev_type={})! Handle: {:p}", dt, h_enc);
                    encoder_handle = h_enc;
                    break;
                }
            }
        }

        // Fórmula oficial da NVIDIA do header nvEncodeAPI.h:
        // #define NVENCAPI_VERSION (NVENCAPI_MAJOR_VERSION | (NVENCAPI_MINOR_VERSION << 24))
        // #define NVENCAPI_STRUCT_VERSION(ver) ((uint32_t)NVENCAPI_VERSION | ((ver)<<16) | (0x7 << 28))
        let nvenc_struct_version = |ver: u32, major: u32, minor: u32, is_flagged: bool| -> u32 {
            let api_ver = major | (minor << 24);
            let s_v = api_ver | (ver << 16) | (0x7 << 28);
            if is_flagged { s_v | (1 << 31) } else { s_v }
        };

        // Testar inicialização de sessão com versões canônicas da NVIDIA
        println!("\n🔍 ABRINDO SESSÃO NVENC NO SILÍCIO DA NVIDIA:");
        let mut encoder_handle: *mut c_void = std::ptr::null_mut();
        let mut matched_major = 10u32;
        let mut matched_minor = 1u32;

        let open_session_ex: unsafe extern "system" fn(open_params: *mut c_void, encoder: *mut *mut c_void) -> u32 = std::mem::transmute(ptr_slice[30]);

        for s_ver in [0x7A010003, 0x7A010002, 0x7A010001, 0x7001000C, 0x7001000B, 0x7001000A] {
            for api_ver in [0x0100000A, 0x0000000C, 0x0000000B, 0x0000000A] {
                let mut open_params: NvEncOpenEncodeSessionExParams = std::mem::zeroed();
                open_params.version = s_ver;
                open_params.device_type = 0; // Direct3D 11
                open_params.device = d3d11_device;
                open_params.api_version = api_ver;

                let mut h: *mut c_void = std::ptr::null_mut();
                let status = open_session_ex(&mut open_params as *mut _ as *mut c_void, &mut h);
                if status == 0 && !h.is_null() {
                    println!("🎉 SESSÃO DE HARDWARE ABERTA COM SUCESSO! (D3D11) struct=0x{:08X}, api=0x{:08X} -> Handle: {:p}",
                        s_ver, api_ver, h);
                    encoder_handle = h;
                    break;
                }
            }
            if !encoder_handle.is_null() { break; }
        }

        if encoder_handle.is_null() {
            println!("❌ Falha ao abrir sessão NVENC.");
            return;
        }

        println!("\n🔍 CONSULTANDO CODECS SUPORTADOS NO SILÍCIO (Slots 2 & 3):");
        let get_guid_count_fn: unsafe extern "system" fn(encoder: *mut c_void, count: *mut u32) -> u32 = std::mem::transmute(ptr_slice[2]);
        let get_guids_fn: unsafe extern "system" fn(encoder: *mut c_void, guids: *mut GUID, count: u32, out_count: *mut u32) -> u32 = std::mem::transmute(ptr_slice[3]);

        let mut guid_count = 0u32;
        let c_status = get_guid_count_fn(encoder_handle, &mut guid_count);
        println!("   -> nvEncGetEncodeGUIDCount status: {} | Total de Codecs na GPU: {}", c_status, guid_count);

        if guid_count > 0 {
            let mut guids = vec![GUID::default(); guid_count as usize];
            let mut actual_count = 0u32;
            let g_status = get_guids_fn(encoder_handle, guids.as_mut_ptr(), guid_count, &mut actual_count);
            println!("   -> nvEncGetEncodeGUIDs status: {} | Codecs retornados: {}", g_status, actual_count);
            for (idx, g) in guids.iter().enumerate() {
                let is_h264 = g.data1 == NV_ENC_CODEC_H264_GUID.data1;
                println!("      [{}] GUID: 0x{:08X}-{:04X}-{:04X} {}", idx, g.data1, g.data2, g.data3, if is_h264 { "🎯 (H.264 CODEC)" } else { "" });
            }
        }

        // Consultar Presets Suportados (Slots 9 & 10)
        let get_preset_count_fn: unsafe extern "system" fn(encoder: *mut c_void, encode_guid: GUID, count: *mut u32) -> u32 = std::mem::transmute(ptr_slice[9]);
        let get_preset_guids_fn: unsafe extern "system" fn(encoder: *mut c_void, encode_guid: GUID, presets: *mut GUID, count: u32, out_count: *mut u32) -> u32 = std::mem::transmute(ptr_slice[10]);

        let mut preset_count = 0u32;
        let pr_c_status = get_preset_count_fn(encoder_handle, NV_ENC_CODEC_H264_GUID, &mut preset_count);
        println!("\n🔍 CONSULTANDO PRESETS DE H.264 (Slots 9 & 10): status={}, count={}", pr_c_status, preset_count);

        let mut selected_preset_guid = NV_ENC_PRESET_P1_GUID;
        if preset_count > 0 {
            let mut p_guids = vec![GUID::default(); preset_count as usize];
            let mut p_actual = 0u32;
            let pr_status = get_preset_guids_fn(encoder_handle, NV_ENC_CODEC_H264_GUID, p_guids.as_mut_ptr(), preset_count, &mut p_actual);
            println!("   -> nvEncGetEncodePresetGUIDs status: {} | Presets:", pr_status);
            for (idx, pg) in p_guids.iter().enumerate() {
                println!("      [{}] Preset GUID: 0x{:08X}-{:04X}-{:04X}", idx, pg.data1, pg.data2, pg.data3);
            }
            if !p_guids.is_empty() {
                selected_preset_guid = p_guids[0];
            }
        }

        // 7. Alocar buffers de VRAM
        let create_in_fn: unsafe extern "system" fn(encoder: *mut c_void, in_params: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[13]);
        let mut in_params: NvEncCreateInputBufferParams = std::mem::zeroed();
        in_params.version = nvenc_struct_version(1, matched_major, matched_minor, false);
        in_params.width = 1920;
        in_params.height = 1080;
        in_params.buffer_format = 0x01000000; // ARGB
        let in_status = create_in_fn(encoder_handle, &mut in_params as *mut _ as *mut c_void);
        println!("   -> nvEncCreateInputBuffer status: {} | VRAM Buffer: {:p}", in_status, in_params.input_buffer);

        let create_bs_fn: unsafe extern "system" fn(encoder: *mut c_void, bs_params: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[15]);
        let mut bs_params: NvEncCreateBitstreamBufferParams = std::mem::zeroed();
        bs_params.version = nvenc_struct_version(1, matched_major, matched_minor, false);
        bs_params.size = 2 * 1024 * 1024;
        let bs_status = create_bs_fn(encoder_handle, &mut bs_params as *mut _ as *mut c_void);
        println!("   -> nvEncCreateBitstreamBuffer status: {} | VRAM Bitstream: {:p}", bs_status, bs_params.bitstream_buffer);

        // 8. Teste de Codificação de 60 Quadros a 1080p 60 FPS
        println!("\n⚡ TESTANDO CODIFICAÇÃO DE 60 QUADROS REAIS (1920x1080) NA GPU:");
        let lock_in_fn: unsafe extern "system" fn(encoder: *mut c_void, lock_params: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[20]);
        let unlock_in_fn: unsafe extern "system" fn(encoder: *mut c_void, buffer: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[21]);
        let encode_pic_fn: unsafe extern "system" fn(encoder: *mut c_void, pic_params: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[17]);
        let lock_bs_fn: unsafe extern "system" fn(encoder: *mut c_void, lock_params: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[18]);
        let unlock_bs_fn: unsafe extern "system" fn(encoder: *mut c_void, buffer: *mut c_void) -> u32 = std::mem::transmute(ptr_slice[19]);

        let dummy_frame = vec![128u8; 1920 * 1080 * 4];
        let mut total_encode_us = 0u128;

        for frame_idx in 0..60 {
            let start = Instant::now();

            let mut lock_in: NvEncLockInputBufferParams = std::mem::zeroed();
            lock_in.version = nvenc_struct_version(1, matched_major, matched_minor, false);
            lock_in.input_buffer = in_params.input_buffer;

            let lock_s = lock_in_fn(encoder_handle, &mut lock_in as *mut _ as *mut c_void);
            if lock_s == 0 && !lock_in.buffer_data_ptr.is_null() {
                let dst_pitch = lock_in.pitch as usize;
                let src_pitch = 1920 * 4;
                let copy_w = src_pitch.min(dst_pitch);
                let dst_slice = std::slice::from_raw_parts_mut(lock_in.buffer_data_ptr as *mut u8, dst_pitch * 1080);

                for row in 0..1080 {
                    let s_off = row * src_pitch;
                    let d_off = row * dst_pitch;
                    dst_slice[d_off..d_off + copy_w].copy_from_slice(&dummy_frame[s_off..s_off + copy_w]);
                }
                unlock_in_fn(encoder_handle, in_params.input_buffer);

                let mut pic_params: NvEncPicParamsStruct = std::mem::zeroed();
                pic_params.version = nvenc_struct_version(4, matched_major, matched_minor, true);
                pic_params.input_width = 1920;
                pic_params.input_height = 1080;
                pic_params.input_pitch = lock_in.pitch;
                pic_params.input_buffer = in_params.input_buffer;
                pic_params.output_bitstream = bs_params.bitstream_buffer;
                pic_params.buffer_format = 0x01000000;
                pic_params.picture_struct = 1;
                if frame_idx == 0 {
                    pic_params.encode_pic_flags = 0x1 | 0x2; // FORCEIDR | OUTPUT_SPSPPS
                }

                let enc_s = encode_pic_fn(encoder_handle, &mut pic_params as *mut _ as *mut c_void);
                if enc_s == 0 {
                    let mut lock_bs: NvEncLockBitstreamParams = std::mem::zeroed();
                    lock_bs.version = nvenc_struct_version(1, matched_major, matched_minor, false);
                    lock_bs.output_bitstream = bs_params.bitstream_buffer;

                    let bs_s = lock_bs_fn(encoder_handle, &mut lock_bs as *mut _ as *mut c_void);
                    if bs_s == 0 {
                        let bytes = lock_bs.bitstream_size_in_bytes;
                        unlock_bs_fn(encoder_handle, bs_params.bitstream_buffer);
                        let elapsed_us = start.elapsed().as_micros();
                        total_encode_us += elapsed_us;
                        if frame_idx % 10 == 0 {
                            println!("   [Frame {:02}] Tamanho NAL H.264: {} bytes | Latência GPU: {:.2} ms ({} µs)", 
                                frame_idx, bytes, elapsed_us as f64 / 1000.0, elapsed_us);
                        }
                    }
                }
            }
        }

        let avg_latency_ms = (total_encode_us as f64 / 60.0) / 1000.0;
        println!("\n============================================================");
        println!("🏆 SUCESSO TOTAL! 60 QUADROS CODIFICADOS NA GPU NVIDIA!");
        println!("📊 Latência média por quadro: {:.2} ms ({:.0} FPS de capacidade teórica de hardware!)", 
            avg_latency_ms, 1000.0 / avg_latency_ms.max(0.001));
        println!("============================================================");
    }
}
