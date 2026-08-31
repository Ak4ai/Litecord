use std::mem::ManuallyDrop;
use std::time::Instant;
use windows::core::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

fn main() -> Result<()> {
    println!("============================================================");
    println!("🚀 TESTE: HARDWARE DIRECT MFT ENCODER (IMFTransform)");
    println!("============================================================");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let hr_start = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        println!("🚀 MFStartup(MF_VERSION, MFSTARTUP_LITE): {:?}", hr_start);

        let mut in_type = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let mut out_type = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };

        let mut pp_activate: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;

        let hr = MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&in_type),
            Some(&out_type),
            &mut pp_activate,
            &mut count,
        );

        println!("👉 MFTEnumEx (SYNCMFT): {:?} (Count: {})", hr, count);

        if count == 0 || pp_activate.is_null() {
            println!("⚠️ Nenhum MFT de hardware encontrado com filtro NV12. Tentando sem filtro de entrada...");
            let _ = MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                None,
                Some(&out_type),
                &mut pp_activate,
                &mut count,
            );
            println!("👉 MFTEnumEx genérico: Count = {}", count);
        }

        if count > 0 && !pp_activate.is_null() {
            let activates = std::slice::from_raw_parts(pp_activate, count as usize);
            for (idx, act_opt) in activates.iter().enumerate() {
                if let Some(ref act) = act_opt {
                    let mut name_buf = [0u16; 128];
                    let _ = act.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut name_buf, None);
                    let name = String::from_utf16_lossy(&name_buf).trim_matches('\0').trim().to_string();
                    println!("--------------------------------------------------");
                    println!("🎯 Testando MFT #{}: '{}'", idx, name);

                    let transform_res = act.ActivateObject::<IMFTransform>();
                    if let Ok(transform) = transform_res {
                        println!("✅ IMFTransform ativado com sucesso!");

                        if let Ok(mut attrs) = transform.GetAttributes() {
                            let _ = attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
                            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
                        }

                        fn set_size(attrs: &IMFAttributes, key: &windows::core::GUID, w: u32, h: u32) -> Result<()> {
                            unsafe { attrs.SetUINT64(key, ((w as u64) << 32) | (h as u64)) }
                        }
                        fn set_ratio(attrs: &IMFAttributes, key: &windows::core::GUID, num: u32, den: u32) -> Result<()> {
                            unsafe { attrs.SetUINT64(key, ((num as u64) << 32) | (den as u64)) }
                        }

                        // Configurar Saída H.264
                        let out_media_type = MFCreateMediaType()?;
                        out_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                        out_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
                        out_media_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)?;
                        set_size(&out_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
                        set_ratio(&out_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
                        set_ratio(&out_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
                        out_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                        out_media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;

                        let hr_out = transform.SetOutputType(0, &out_media_type, 0);
                        println!("👉 SetOutputType (H264 1080p 60 FPS): {:?}", hr_out);

                        // Configurar Entrada NV12
                        let in_media_type = MFCreateMediaType()?;
                        in_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                        in_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                        set_size(&in_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
                        set_ratio(&in_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
                        set_ratio(&in_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
                        in_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

                        let hr_in = transform.SetInputType(0, &in_media_type, 0);
                        println!("👉 SetInputType (NV12 1080p 60 FPS): {:?}", hr_in);

                        if hr_out.is_ok() && hr_in.is_ok() {
                            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
                            let mut d3d11_device: Option<ID3D11Device> = None;
                            let mut feature_level = D3D_FEATURE_LEVEL_11_0;
                            D3D11CreateDevice(
                                None,
                                D3D_DRIVER_TYPE_HARDWARE,
                                None,
                                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                                None,
                                D3D11_SDK_VERSION,
                                Some(&mut d3d11_device),
                                Some(&mut feature_level),
                                None,
                            )?;
                            if let Some(dev) = d3d11_device {
                                let mut reset_token = 0u32;
                                let mut device_manager: Option<IMFDXGIDeviceManager> = None;
                                MFCreateDXGIDeviceManager(&mut reset_token, &mut device_manager)?;
                                if let Some(dm) = device_manager {
                                    dm.ResetDevice(&dev, reset_token)?;
                                    let dm_unknown: IUnknown = dm.cast()?;
                                    let hr_msg = transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, dm_unknown.as_raw() as usize);
                                    println!("🎮 MFT_MESSAGE_SET_D3D_MANAGER (Hardware D3D11): {:?}", hr_msg);
                                }
                            }

                            let _ = transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
                            let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
                            let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);

                            println!("🚀 Pipeline MFT pronta e iniciada!");

                            let nv12_size = 1920 * 1080 + (1920 * 1080 / 2);
                            let dummy_nv12 = vec![128u8; nv12_size];

                            let in_buffer = MFCreateMemoryBuffer(nv12_size as u32)?;
                            let mut p_buf_data: *mut u8 = std::ptr::null_mut();
                            let mut max_len = 0u32;
                            let mut cur_len = 0u32;
                            in_buffer.Lock(&mut p_buf_data, Some(&mut max_len), Some(&mut cur_len))?;
                            std::ptr::copy_nonoverlapping(dummy_nv12.as_ptr(), p_buf_data, nv12_size);
                            in_buffer.Unlock()?;
                            in_buffer.SetCurrentLength(nv12_size as u32)?;

                            let in_sample = MFCreateSample()?;
                            in_sample.AddBuffer(&in_buffer)?;
                            in_sample.SetSampleTime(0)?;
                            in_sample.SetSampleDuration(166_666)?;

                            let t0 = Instant::now();
                            let hr_input = transform.ProcessInput(0, &in_sample, 0);
                            let dur_in = t0.elapsed();
                            println!("👉 ProcessInput: {:?} (Tempo: {:?})", hr_input, dur_in);

                            let stream_info = transform.GetOutputStreamInfo(0).unwrap_or(MFT_OUTPUT_STREAM_INFO::default());
                            let out_buf_size = stream_info.cbSize.max(1024 * 1024);
                            println!("📊 Output Stream cbSize: {} bytes", stream_info.cbSize);

                            let out_buffer = MFCreateMemoryBuffer(out_buf_size)?;
                            let out_sample = MFCreateSample()?;
                            out_sample.AddBuffer(&out_buffer)?;

                            let mut out_data_buffer = [MFT_OUTPUT_DATA_BUFFER {
                                dwStreamID: 0,
                                pSample: ManuallyDrop::new(Some(out_sample)),
                                dwStatus: 0,
                                pEvents: ManuallyDrop::new(None),
                            }];
                            let mut status = 0u32;

                            let t1 = Instant::now();
                            let hr_output = transform.ProcessOutput(0, &mut out_data_buffer, &mut status);
                            let dur_out = t1.elapsed();

                            println!("👉 ProcessOutput: {:?} (Tempo: {:?})", hr_output, dur_out);

                            if let Some(ref sample) = *out_data_buffer[0].pSample {
                                let total_len = sample.GetTotalLength().unwrap_or(0);
                                println!("🎉 SUCESSO! H.264 NAL extraído com sucesso! Tamanho: {} bytes", total_len);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
