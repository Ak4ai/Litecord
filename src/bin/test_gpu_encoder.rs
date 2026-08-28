#[repr(C)]
struct NvEncodeApiFunctionList {
    version: u32,
    reserved: u32,
    nv_enc_open_encode_session: usize,
    nv_enc_get_encode_guid_count: usize,
    nv_enc_get_encode_profile_guid_count: usize,
    nv_enc_get_encode_profile_guids: usize,
    nv_enc_get_encode_guids: usize,
    nv_enc_get_input_format_count: usize,
    nv_enc_get_input_formats: usize,
    nv_enc_get_encode_caps: usize,
    nv_enc_get_encode_preset_count: usize,
    nv_enc_get_encode_preset_guids: usize,
    nv_enc_get_encode_preset_config: usize,
    nv_enc_initialize_encoder: usize,
    nv_enc_create_input_buffer: usize,
    nv_enc_destroy_input_buffer: usize,
    nv_enc_create_bitstream_buffer: usize,
    nv_enc_destroy_bitstream_buffer: usize,
    nv_enc_encode_picture: usize,
    nv_enc_lock_bitstream: usize,
    nv_enc_unlock_bitstream: usize,
    nv_enc_lock_input_buffer: usize,
    nv_enc_unlock_input_buffer: usize,
    nv_enc_get_encode_stats: usize,
    nv_enc_get_sequence_params: usize,
    nv_enc_register_async_event: usize,
    nv_enc_unregister_async_event: usize,
    nv_enc_map_input_resource: usize,
    nv_enc_unmap_input_resource: usize,
    nv_enc_destroy_encoder: usize,
    nv_enc_invalidate_ref_frames: usize,
    nv_enc_open_encode_session_ex: usize,
    nv_enc_register_resource: usize,
    nv_enc_unregister_resource: usize,
    nv_enc_reconfigure_encoder: usize,
    reserved1: [usize; 288],
}

fn test_nvenc() {
    #[cfg(windows)]
    unsafe {
        let nvenc_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"nvEncodeAPI64.dll\0".as_ptr());
        if nvenc_dll.is_null() {
            println!("  NVIDIA NVENC: ❌ Não disponível nesta máquina.");
            return;
        }

        let get_proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        let get_version_fn: Option<unsafe extern "system" fn(*mut u32) -> u32> =
            std::mem::transmute(get_proc(nvenc_dll, b"NvEncodeAPIGetMaxSupportedVersion\0".as_ptr()));
        let create_instance_fn: Option<unsafe extern "system" fn(*mut NvEncodeApiFunctionList) -> u32> =
            std::mem::transmute(get_proc(nvenc_dll, b"NvEncodeAPICreateInstance\0".as_ptr()));

        if let (Some(get_version), Some(create_instance)) = (get_version_fn, create_instance_fn) {
            let mut max_version = 0u32;
            let _ = get_version(&mut max_version);
            let major = max_version >> 4;
            let minor = max_version & 0xF;

            let nvenc_api_ver = major | (minor << 24);
            let struct_ver = nvenc_api_ver | (2 << 16) | (0x7 << 28);
            let mut fn_list: NvEncodeApiFunctionList = std::mem::zeroed();
            fn_list.version = struct_ver;

            if create_instance(&mut fn_list) == 0 {
                println!("  NVIDIA NVENC (GeForce): ✅ 100% OPERACIONAL! (API v{}.{}, Endereço Fn: 0x{:X})", 
                    major, minor, fn_list.nv_enc_encode_picture);
            }
        }
    }
}

fn test_amf() {
    #[cfg(windows)]
    unsafe {
        let amf_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"amfrt64.dll\0".as_ptr());
        if amf_dll.is_null() {
            println!("  AMD AMF (Radeon):      ❌ Não disponível nesta máquina (Disponível no PC Desktop).");
            return;
        }

        let get_proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        let amf_query_version_fn: Option<unsafe extern "system" fn(*mut u64) -> i32> =
            std::mem::transmute(get_proc(amf_dll, b"AMFQueryVersion\0".as_ptr()));

        if let Some(query_version) = amf_query_version_fn {
            let mut version = 0u64;
            if query_version(&mut version) == 0 {
                let major = (version >> 48) & 0xFFFF;
                let minor = (version >> 32) & 0xFFFF;
                let subminor = (version >> 16) & 0xFFFF;
                println!("  AMD AMF (Radeon):      ✅ 100% OPERACIONAL! (Versão {}.{}.{})", major, minor, subminor);
            }
        }
    }
}

fn main() {
    println!("============================================================");
    println!("🚀 VERIFICAÇÃO DE ENCODERS DE HARDWARE GPU (ESTILO SUNSHINE)");
    println!("============================================================");
    test_nvenc();
    test_amf();
    println!("============================================================");
}
