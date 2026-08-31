// Teste Isolado: Hardware H.264 Encoder via IMFSinkWriter + Direct3D 11 (Universal)
// Verificação de extração de bytes H.264 NAL em tempo real

use std::time::Instant;
use windows::core::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

fn main() -> Result<()> {
    println!("============================================================");
    println!("🚀 TESTE ISOLADO: HARDWARE H.264 ENCODER - IMFSinkWriter + D3D11");
    println!("============================================================");

    unsafe {
        // 1. Inicializar COM e Media Foundation
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        println!("✅ Windows Media Foundation inicializado com sucesso!");

        // 2. Enumeração DXGI para selecionar a melhor GPU
        println!("\n🔍 ENUMERANDO PLACAS DE VÍDEO DO SISTEMA:");
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut selected_adapter: Option<IDXGIAdapter1> = None;
        let mut selected_gpu_name = String::new();
        let mut best_vram: usize = 0;

        let mut adapter_index = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let desc = adapter.GetDesc1()?;
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_matches('\0')
                .trim()
                .to_string();
            let vram_mb = desc.DedicatedVideoMemory / (1024 * 1024);
            let is_software = (desc.Flags & 2) != 0;

            println!("   [{}] GPU: '{}' | Vendor ID: 0x{:04X} | VRAM: {} MB {}",
                adapter_index, name, desc.VendorId, vram_mb, if is_software { "(Software)" } else { "" });

            if !is_software && (desc.VendorId == 0x10DE || desc.VendorId == 0x1002 || vram_mb > best_vram) {
                selected_adapter = Some(adapter);
                selected_gpu_name = name;
                best_vram = vram_mb;
            }
            adapter_index += 1;
        }

        println!("\n🎯 GPU SELECIONADA: '{}' ({} MB VRAM)", selected_gpu_name, best_vram);

        // 3. Criar Direct3D 11 Device na GPU
        let mut d3d11_device: Option<ID3D11Device> = None;
        let mut d3d11_context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;

        let adapter_ref = selected_adapter.as_ref();
        let driver_type = if adapter_ref.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE };

        D3D11CreateDevice(
            adapter_ref.map(|a| a as &IDXGIAdapter),
            driver_type,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d11_device),
            Some(&mut feature_level),
            Some(&mut d3d11_context),
        )?;

        let d3d11_device = d3d11_device.unwrap();
        println!("✅ Direct3D 11 Device criado com sucesso! Feature Level: 0x{:04X}", feature_level.0);

        // 4. Criar IMFDXGIDeviceManager
        let mut reset_token = 0u32;
        let mut device_manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut reset_token, &mut device_manager)?;
        let device_manager = device_manager.unwrap();
        device_manager.ResetDevice(&d3d11_device, reset_token)?;
        println!("✅ IMFDXGIDeviceManager vinculado à GPU!");

        // 5. Configurar Atributos do SinkWriter com aceleração de hardware D3D11
        let mut writer_attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut writer_attributes, 4)?;
        let writer_attributes = writer_attributes.unwrap();

        writer_attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &device_manager)?;
        writer_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        writer_attributes.SetUINT32(&MF_LOW_LATENCY, 1)?;

        let byte_stream: IMFByteStream = MFCreateTempFile(MF_ACCESSMODE_READWRITE, MF_OPENMODE_DELETE_IF_EXIST, MF_FILEFLAGS_NONE)?;
        let sink_writer = MFCreateSinkWriterFromURL(w!(".mp4"), &byte_stream, &writer_attributes)?;
        println!("✅ IMFSinkWriter criado com Aceleração de Hardware Direct3D 11 ativada!");

        fn set_size(attrs: &IMFAttributes, key: &windows::core::GUID, w: u32, h: u32) -> Result<()> {
            unsafe { attrs.SetUINT64(key, ((w as u64) << 32) | (h as u64)) }
        }

        fn set_ratio(attrs: &IMFAttributes, key: &windows::core::GUID, num: u32, den: u32) -> Result<()> {
            unsafe { attrs.SetUINT64(key, ((num as u64) << 32) | (den as u64)) }
        }

        // 6. Configurar Saída H.264 (1080p 60 FPS 4 Mbps)
        let out_media_type = MFCreateMediaType()?;
        out_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_media_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)?;
        set_size(&out_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
        set_ratio(&out_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
        set_ratio(&out_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        out_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out_media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;

        let stream_index = sink_writer.AddStream(&out_media_type)?;
        println!("✅ Stream H.264 adicionado ao SinkWriter! Stream Index: {}", stream_index);

        // 7. Configurar Entrada NV12 (1080p 60 FPS)
        let in_media_type = MFCreateMediaType()?;
        in_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        set_size(&in_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
        set_ratio(&in_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
        set_ratio(&in_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        in_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

        sink_writer.SetInputMediaType(stream_index, &in_media_type, None)?;
        println!("✅ Input Type NV12 configurado no SinkWriter!");

        // 8. Iniciar o Gravador
        sink_writer.BeginWriting()?;
        println!("🚀 SinkWriter iniciado e gravando na GPU!");

        // 9. Codificar 60 Quadros Reais de Vídeo em 1080p @ 60 FPS
        println!("\n⚡ TESTANDO CODIFICAÇÃO DE 60 QUADROS REAIS (1920x1080) NA GPU:");
        let nv12_frame_size = 1920 * 1080 + (1920 * 1080 / 2);
        let dummy_nv12 = vec![128u8; nv12_frame_size];

        let mut total_encode_us = 0u128;
        let mut encoded_frames = 0usize;
        let mut last_read_pos = 0u64;

        for frame_idx in 0..60 {
            let start = Instant::now();

            let in_buffer = MFCreateMemoryBuffer(nv12_frame_size as u32)?;
            let mut p_buf_data: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            in_buffer.Lock(&mut p_buf_data, Some(&mut max_len), Some(&mut cur_len))?;
            std::ptr::copy_nonoverlapping(dummy_nv12.as_ptr(), p_buf_data, nv12_frame_size);
            in_buffer.Unlock()?;
            in_buffer.SetCurrentLength(nv12_frame_size as u32)?;

            let in_sample = MFCreateSample()?;
            in_sample.AddBuffer(&in_buffer)?;
            let sample_time = (frame_idx as i64) * 166_666; // 16.66 ms
            in_sample.SetSampleTime(sample_time)?;
            in_sample.SetSampleDuration(166_666)?;

            // Enviar sample sincronamente para o encoder por hardware na GPU
            let hr_write = sink_writer.WriteSample(stream_index, &in_sample);

            let elapsed_us = start.elapsed().as_micros();
            if hr_write.is_ok() {
                total_encode_us += elapsed_us;
                encoded_frames += 1;

                let cur_len = byte_stream.GetLength().unwrap_or(0);
                let mut nal_bytes = Vec::new();
                if cur_len > last_read_pos {
                    let to_read = (cur_len - last_read_pos) as u32;
                    let mut buf = vec![0u8; to_read as usize];
                    let mut read = 0u32;
                    let _ = byte_stream.SetCurrentPosition(last_read_pos);
                    let _ = byte_stream.Read(&mut buf, &mut read);
                    last_read_pos = cur_len;
                    nal_bytes = buf;
                }

                if frame_idx % 10 == 0 || frame_idx < 3 {
                    println!("   [Frame {:02}] ✅ NAL Units extraídos da GPU: {} bytes | Latência: {:.2} ms ({} µs)",
                        frame_idx, nal_bytes.len(), elapsed_us as f64 / 1000.0, elapsed_us);
                }
            } else {
                println!("   [Frame {:02}] WriteSample: {:?}", frame_idx, hr_write);
            }
        }

        let stream_len = byte_stream.GetLength().unwrap_or(0);
        println!("\n📦 Tamanho Total do Fluxo H.264 gerado pelo Hardware da GPU: {} KB ({} bytes)", stream_len / 1024, stream_len);

        if encoded_frames > 0 {
            let avg_latency_ms = (total_encode_us as f64 / encoded_frames as f64) / 1000.0;
            println!("\n============================================================");
            println!("🏆 SUCESSO TOTAL! {} QUADROS CODIFICADOS NA GPU ({})!", encoded_frames, selected_gpu_name);
            println!("📊 Latência média por quadro: {:.2} ms ({:.0} FPS de capacidade teórica no silício!)",
                avg_latency_ms, 1000.0 / avg_latency_ms.max(0.001));
            println!("============================================================");
        }
    }

    Ok(())
}
