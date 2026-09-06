// ============================================================================
// 🚀 LITECORD GPU HARDWARE ACCELERATED VIDEO ENCODER ENGINE (SUNSHINE-GRADE)
// ============================================================================
// Provedor unificado de codificação de vídeo com suporte a:
// 1. NVIDIA NVENC (GeForce / Quadro Hardware Acceleration via nvEncodeAPI64 / FFmpeg)
// 2. AMD AMF (Radeon Hardware Acceleration via amfrt64 Zero-Copy)
// 3. Windows Media Foundation D3D11 (Universal Windows GPU Acceleration)
// 4. OpenH264 SIMD Multi-threading com Rayon Paralelizado (Fallback Universal CPU)
// 5. Bitrate Dinâmico Adaptativo (Sunshine/WebRTC AIMD Rate Control)
// ============================================================================
#![allow(dead_code, non_snake_case, non_camel_case_types)]

pub mod nvenc;
pub use nvenc as ffmpeg_nvenc;

#[cfg(target_os = "windows")]
pub mod amd_amf;

#[cfg(target_os = "windows")]
pub mod wmf;

pub mod software;

pub use nvenc::FfmpegNvencEncoder;
#[cfg(target_os = "windows")]
pub use amd_amf::AmdAmfZeroCopyEncoder;
#[cfg(target_os = "windows")]
pub use wmf::WmfGpuEncoder;
pub use software::OpenH264Encoder;

use log::{info, warn};

/// Interface unificada para qualquer engine de codificação de vídeo
pub trait VideoEncoder: Send {
    /// Codifica um frame BGRA para stream H.264 NAL units
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>>;

    /// Força a geração imediata de um IDR / Keyframe (PLI Recovery)
    fn force_intra_frame(&mut self);

    /// Ajusta dinamicamente a taxa de bits (Adaptive Bitrate)
    fn set_bitrate_bps(&mut self, bitrate_bps: u32);

    /// Retorna a taxa de bits atual configurada em bps
    fn get_bitrate_bps(&self) -> u32;

    /// Retorna o nome amigável do backend ativo
    fn name(&self) -> &'static str;

    /// Indica se está rodando em hardware dedicado de GPU
    fn is_hardware_accelerated(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuHardwareType {
    NvidiaNvenc,
    AmdAmf,
    SoftwareOnly,
}

pub fn detect_gpu_hardware() -> GpuHardwareType {
    #[cfg(target_os = "windows")]
    unsafe {
        let nvenc_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"nvEncodeAPI64.dll\0".as_ptr());
        if !nvenc_dll.is_null() {
            let get_proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress;
            let create_instance_fn = get_proc(nvenc_dll, b"NvEncodeAPICreateInstance\0".as_ptr());
            if create_instance_fn.is_some() {
                windows_sys::Win32::Foundation::FreeLibrary(nvenc_dll);
                return GpuHardwareType::NvidiaNvenc;
            }
            windows_sys::Win32::Foundation::FreeLibrary(nvenc_dll);
        }

        let amf_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"amfrt64.dll\0".as_ptr());
        if !amf_dll.is_null() {
            let get_proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress;
            let amf_query_version_fn = get_proc(amf_dll, b"AMFQueryVersion\0".as_ptr());
            if amf_query_version_fn.is_some() {
                windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
                return GpuHardwareType::AmdAmf;
            }
            windows_sys::Win32::Foundation::FreeLibrary(amf_dll);
        }
    }

    #[cfg(target_os = "linux")]
    unsafe {
        for nv_name in [b"libnvidia-encode.so.1\0".as_ptr(), b"libnvidia-encode.so\0".as_ptr()] {
            let h = libc::dlopen(nv_name as *const libc::c_char, libc::RTLD_LAZY | libc::RTLD_LOCAL);
            if !h.is_null() {
                let sym = libc::dlsym(h, b"NvEncodeAPICreateInstance\0".as_ptr() as *const libc::c_char);
                libc::dlclose(h);
                if !sym.is_null() {
                    return GpuHardwareType::NvidiaNvenc;
                }
            }
        }

        for amf_name in [b"libamfrt64.so.1\0".as_ptr(), b"libamfrt64.so\0".as_ptr()] {
            let h = libc::dlopen(amf_name as *const libc::c_char, libc::RTLD_LAZY | libc::RTLD_LOCAL);
            if !h.is_null() {
                let sym = libc::dlsym(h, b"AMFQueryVersion\0".as_ptr() as *const libc::c_char);
                libc::dlclose(h);
                if !sym.is_null() {
                    return GpuHardwareType::AmdAmf;
                }
            }
        }
    }

    GpuHardwareType::SoftwareOnly
}

/// Cria a melhor instância de codificador de vídeo disponível para a plataforma
pub fn create_best_encoder(target_fps: u32, is_screen_content: bool) -> Box<dyn VideoEncoder> {
    info!("🔍 [VIDEO CODEC FACTORY] Avaliando melhor engine de codificação para o sistema...");

    let manual_selection = crate::video_settings::get_video_encoder();
    if manual_selection != "auto" {
        info!("🎯 [VIDEO CODEC FACTORY] Codificador manual configurado pelo usuário: '{}'", manual_selection);
        match manual_selection.as_str() {
            "nvenc" => {
                match nvenc::FfmpegNvencEncoder::try_new_with_codec(target_fps, is_screen_content, Some("nvenc")) {
                    Ok(enc) => {
                        info!("🚀 [VIDEO CODEC FACTORY] Codificador manual NVIDIA NVENC ativado com sucesso!");
                        return Box::new(enc);
                    }
                    Err(e) => warn!("⚠️ [VIDEO CODEC FACTORY] NVENC manual indisponível ({}), executando detecção automática...", e),
                }
            }
            "amf" => {
                #[cfg(target_os = "windows")]
                {
                    match amd_amf::AmdAmfZeroCopyEncoder::try_new(target_fps, is_screen_content) {
                        Ok(enc) => {
                            info!("🚀 [VIDEO CODEC FACTORY] Codificador manual AMD AMF Zero-Copy ativado com sucesso!");
                            return Box::new(enc);
                        }
                        Err(e) => warn!("⚠️ [VIDEO CODEC FACTORY] AMF Zero-Copy manual falhou ({}), tentando h264_amf...", e),
                    }
                }
                match nvenc::FfmpegNvencEncoder::try_new_with_codec(target_fps, is_screen_content, Some("amf")) {
                    Ok(enc) => {
                        info!("🚀 [VIDEO CODEC FACTORY] Codificador manual AMD AMF ativado com sucesso!");
                        return Box::new(enc);
                    }
                    Err(e) => warn!("⚠️ [VIDEO CODEC FACTORY] AMF manual indisponível ({}), executando detecção automática...", e),
                }
            }
            "ffmpeg" => {
                match nvenc::FfmpegNvencEncoder::try_new(target_fps, is_screen_content) {
                    Ok(enc) => {
                        info!("🚀 [VIDEO CODEC FACTORY] Codificador manual FFmpeg GPU ativado com sucesso!");
                        return Box::new(enc);
                    }
                    Err(e) => warn!("⚠️ [VIDEO CODEC FACTORY] FFmpeg GPU manual indisponível ({}), executando detecção automática...", e),
                }
            }
            "wmf" => {
                #[cfg(target_os = "windows")]
                {
                    match wmf::WmfGpuEncoder::try_new(target_fps, is_screen_content) {
                        Ok(enc) => {
                            info!("🚀 [VIDEO CODEC FACTORY] Codificador manual WMF GPU ativado com sucesso!");
                            return Box::new(enc);
                        }
                        Err(e) => warn!("⚠️ [VIDEO CODEC FACTORY] WMF GPU manual indisponível ({}), executando detecção automática...", e),
                    }
                }
            }
            "openh264" => {
                info!("🎯 [VIDEO CODEC FACTORY] Inicializando Cisco OpenH264 CPU manual...");
                if let Ok(enc) = OpenH264Encoder::new(target_fps, is_screen_content) {
                    info!("🚀 [VIDEO CODEC FACTORY] Cisco OpenH264 CPU manual ativado com sucesso!");
                    return Box::new(enc);
                }
            }
            _ => {}
        }
    }

    #[cfg(target_os = "windows")]
    {
        let hw = detect_gpu_hardware();
        info!("🎯 [VIDEO CODEC FACTORY] Hardware detectado no sistema: {:?}", hw);

        if let GpuHardwareType::AmdAmf = hw {
            info!("🎯 [VIDEO CODEC FACTORY] GPU AMD Detectada! Inicializando AMF Native Zero-Copy Engine (AMFVideoConverter + VCE, 0% CPU)...");
            match amd_amf::AmdAmfZeroCopyEncoder::try_new(target_fps, is_screen_content) {
                Ok(enc) => {
                    info!("🚀 [VIDEO CODEC FACTORY] AMF Native Zero-Copy Engine ativado com sucesso (Placa: {})!", enc.gpu_name);
                    return Box::new(enc);
                }
                Err(e) => {
                    warn!("⚠️ [VIDEO CODEC FACTORY] AMF Native indisponível ({}), usando fallback para FFmpeg h264_amf...", e);
                }
            }
        }

        info!("🎯 [VIDEO CODEC FACTORY] Tentando Hardware GPU Engine via FFmpeg (NVENC / AMF / QSV)...");
        match nvenc::FfmpegNvencEncoder::try_new(target_fps, is_screen_content) {
            Ok(enc) => {
                info!("🚀 [VIDEO CODEC FACTORY] Hardware GPU Engine via FFmpeg ativado com sucesso!");
                return Box::new(enc);
            }
            Err(e) => {
                warn!("⚠️ [VIDEO CODEC FACTORY] FFmpeg GPU indisponível ({}), tentando Windows Media Foundation...", e);
            }
        }

        info!("🎯 [VIDEO CODEC FACTORY] Inicializando Direct3D 11 + Windows Media Foundation GPU Engine...");
        match wmf::WmfGpuEncoder::try_new(target_fps, is_screen_content) {
            Ok(enc) => {
                info!("🚀 [VIDEO CODEC FACTORY] Direct3D 11 + WMF GPU Engine ativado com sucesso (Hardware: {})!", enc.gpu_name);
                return Box::new(enc);
            }
            Err(e) => {
                warn!("⚠️ [VIDEO CODEC FACTORY] WMF GPU Engine indisponível ({}), acionando fallback de segurança...", e);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let hw = detect_gpu_hardware();
        info!("🎯 [VIDEO CODEC FACTORY] Hardware detectado no sistema: {:?}", hw);

        info!("🎯 [VIDEO CODEC FACTORY] Tentando Hardware GPU Engine via FFmpeg (NVENC / AMF / QSV)...");
        match nvenc::FfmpegNvencEncoder::try_new(target_fps, is_screen_content) {
            Ok(enc) => {
                info!("🚀 [VIDEO CODEC FACTORY] Hardware GPU Engine via FFmpeg ativado com sucesso!");
                return Box::new(enc);
            }
            Err(e) => {
                warn!("⚠️ [VIDEO CODEC FACTORY] FFmpeg GPU indisponível ({}), usando fallback OpenH264...", e);
            }
        }
    }

    info!("🎯 [VIDEO CODEC FACTORY] Inicializando OpenH264 SIMD AVX2 (Universal CPU Engine)...");
    match OpenH264Encoder::new(target_fps, is_screen_content) {
        Ok(enc) => {
            info!("🚀 [VIDEO CODEC FACTORY] Encoder {} ativado com sucesso ({} threads, {:.1} Mbps)!",
                enc.name(), enc.num_threads, enc.get_bitrate_bps() as f64 / 1_000_000.0);
            Box::new(enc)
        }
        Err(e) => {
            warn!("⚠️ [VIDEO CODEC FACTORY] Falha ao inicializar OpenH264: {}. Tentando modo básico...", e);
            Box::new(OpenH264Encoder::new(target_fps, false).expect("Falha crítica no encoder H.264"))
        }
    }
}
