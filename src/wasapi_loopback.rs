#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use log::{info, warn};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

pub const IID_IUNKNOWN: GUID = GUID {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

pub const IID_IAUDIOCLIENT: GUID = GUID {
    data1: 0x7cdc33ac,
    data2: 0x7036,
    data3: 0x4568,
    data4: [0xac, 0xfd, 0x27, 0x0e, 0x90, 0x36, 0x32, 0x14],
};

pub const IID_IAUDIOCAPTURECLIENT: GUID = GUID {
    data1: 0xc8adbd64,
    data2: 0xe71e,
    data3: 0x48a0,
    data4: [0xa4, 0xde, 0x18, 0x5c, 0x39, 0x5c, 0xd3, 0x17],
};

pub const IID_IACTIVATEAUDIOINTERFACECOMPLETIONHANDLER: GUID = GUID {
    data1: 0x41baec76,
    data2: 0x44a6,
    data3: 0x4c5b,
    data4: [0x88, 0xb1, 0x83, 0xb9, 0x0a, 0x69, 0x67, 0x32],
};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WAVEFORMATEX {
    pub wFormatTag: u16,
    pub nChannels: u16,
    pub nSamplesPerSec: u32,
    pub nAvgBytesPerSec: u32,
    pub nBlockAlign: u16,
    pub wBitsPerSample: u16,
    pub cbSize: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WAVEFORMATEXTENSIBLE {
    pub Format: WAVEFORMATEX,
    pub Samples: u16,
    pub dwChannelMask: u32,
    pub SubFormat: GUID,
}

pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
pub const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
pub const AUDCLNT_STREAMFLAGS_LOOPBACK: u32 = 0x00020000;
pub const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x00040000;

#[repr(C)]
pub struct AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
    pub TargetProcessId: u32,
    pub ProcessLoopbackMode: u32,
}

#[repr(C)]
pub struct AUDIOCLIENT_ACTIVATION_PARAMS {
    pub ActivationType: u32,
    pub ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
}

#[repr(C)]
pub struct PROPVARIANT {
    pub vt: u16,
    pub wReserved1: u16,
    pub wReserved2: u16,
    pub wReserved3: u16,
    pub cbSize: u32,
    pub _padding: u32,
    pub pBlobData: *mut u8,
}

#[repr(C)]
pub struct IActivateAudioInterfaceAsyncOperationVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub GetActivateResult: unsafe extern "system" fn(this: *mut c_void, hr: *mut i32, ppv: *mut *mut c_void) -> i32,
}

#[repr(C)]
pub struct IActivateAudioInterfaceAsyncOperation {
    pub vtbl: *mut IActivateAudioInterfaceAsyncOperationVtbl,
}

#[repr(C)]
pub struct IActivateAudioInterfaceCompletionHandlerVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub ActivateCompleted: unsafe extern "system" fn(this: *mut c_void, operation: *mut IActivateAudioInterfaceAsyncOperation) -> i32,
}

#[repr(C)]
struct CompletionHandler {
    vtbl: &'static IActivateAudioInterfaceCompletionHandlerVtbl,
    ref_count: AtomicU32,
    event_handle: *mut c_void,
    client_slot: Arc<Mutex<Option<*mut c_void>>>,
}

unsafe extern "system" fn ch_query_interface(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return -2147467261; // E_POINTER
    }
    let guid = *riid;
    if guid_eq(&guid, &IID_IUNKNOWN) || guid_eq(&guid, &IID_IACTIVATEAUDIOINTERFACECOMPLETIONHANDLER) {
        *ppv = this;
        ch_add_ref(this);
        0
    } else {
        *ppv = std::ptr::null_mut();
        -2147467262 // E_NOINTERFACE
    }
}

unsafe extern "system" fn ch_add_ref(this: *mut c_void) -> u32 {
    let handler = &*(this as *mut CompletionHandler);
    handler.ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn ch_release(this: *mut c_void) -> u32 {
    let handler = &*(this as *mut CompletionHandler);
    let prev = handler.ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        let _ = Box::from_raw(this as *mut CompletionHandler);
    }
    prev - 1
}

unsafe extern "system" fn ch_activate_completed(this: *mut c_void, operation: *mut IActivateAudioInterfaceAsyncOperation) -> i32 {
    let handler = &*(this as *mut CompletionHandler);
    if !operation.is_null() {
        let op_vtbl = (*operation).vtbl;
        let mut hr_res: i32 = 0;
        let mut unk: *mut c_void = std::ptr::null_mut();
        let hr = ((*op_vtbl).GetActivateResult)(operation as *mut c_void, &mut hr_res, &mut unk);
        if hr == 0 && hr_res == 0 && !unk.is_null() {
            *handler.client_slot.lock().unwrap() = Some(unk);
        } else {
            warn!("⚠️ [WASAPI LOOPBACK] ActivateCompleted falhou: hr=0x{:08x}, hr_res=0x{:08x}", hr, hr_res);
        }
    }
    if !handler.event_handle.is_null() {
        windows_sys::Win32::System::Threading::SetEvent(handler.event_handle);
    }
    0
}

static CH_VTBL: IActivateAudioInterfaceCompletionHandlerVtbl = IActivateAudioInterfaceCompletionHandlerVtbl {
    QueryInterface: ch_query_interface,
    AddRef: ch_add_ref,
    Release: ch_release,
    ActivateCompleted: ch_activate_completed,
};

#[repr(C)]
pub struct IAudioClientVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Initialize: unsafe extern "system" fn(this: *mut c_void, ShareMode: u32, StreamFlags: u32, BufferDuration: i64, Periodicity: i64, pFormat: *const WAVEFORMATEX, AudioSessionGuid: *const GUID) -> i32,
    pub GetBufferSize: unsafe extern "system" fn(this: *mut c_void, pNumBufferFrames: *mut u32) -> i32,
    pub GetStreamLatency: unsafe extern "system" fn(this: *mut c_void, phnsLatency: *mut i64) -> i32,
    pub GetCurrentPadding: unsafe extern "system" fn(this: *mut c_void, pNumPaddingFrames: *mut u32) -> i32,
    pub IsFormatSupported: unsafe extern "system" fn(this: *mut c_void, ShareMode: u32, pFormat: *const WAVEFORMATEX, ppClosestMatch: *mut *mut WAVEFORMATEX) -> i32,
    pub GetMixFormat: unsafe extern "system" fn(this: *mut c_void, ppDeviceFormat: *mut *mut WAVEFORMATEX) -> i32,
    pub GetDevicePeriod: unsafe extern "system" fn(this: *mut c_void, phnsDefaultPeriod: *mut i64, phnsMinimumPeriod: *mut i64) -> i32,
    pub Start: unsafe extern "system" fn(this: *mut c_void) -> i32,
    pub Stop: unsafe extern "system" fn(this: *mut c_void) -> i32,
    pub Reset: unsafe extern "system" fn(this: *mut c_void) -> i32,
    pub SetEventHandle: unsafe extern "system" fn(this: *mut c_void, eventHandle: *mut c_void) -> i32,
    pub GetService: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
}

#[repr(C)]
pub struct IAudioClient {
    pub vtbl: *mut IAudioClientVtbl,
}

#[repr(C)]
pub struct IAudioCaptureClientVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32,
    pub AddRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub GetBuffer: unsafe extern "system" fn(this: *mut c_void, ppData: *mut *mut u8, pNumFramesToRead: *mut u32, pdwFlags: *mut u32, pu64DevicePosition: *mut u64, pu64QPCPosition: *mut u64) -> i32,
    pub ReleaseBuffer: unsafe extern "system" fn(this: *mut c_void, NumFramesRead: u32) -> i32,
    pub GetNextPacketSize: unsafe extern "system" fn(this: *mut c_void, pNumFramesInNextPacket: *mut u32) -> i32,
}

#[repr(C)]
pub struct IAudioCaptureClient {
    pub vtbl: *mut IAudioCaptureClientVtbl,
}

type FnActivateAudioInterfaceAsync = unsafe extern "system" fn(
    deviceInterfacePath: *const u16,
    riid: *const GUID,
    activationParams: *const PROPVARIANT,
    completionHandler: *mut c_void,
    activationOperation: *mut *mut IActivateAudioInterfaceAsyncOperation,
) -> i32;

pub struct WasapiIsolatedStreamHandle {
    pub sample_rate: u32,
    pub channels: u16,
}

pub fn start_wasapi_isolated_loopback(
    is_running: Arc<AtomicBool>,
    pcm_buffer: Arc<Mutex<Vec<i16>>>,
) -> Result<WasapiIsolatedStreamHandle, String> {
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(
            std::ptr::null_mut(),
            windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
        );

        let mmdevapi = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"mmdevapi.dll\0".as_ptr());
        if mmdevapi.is_null() {
            return Err("Falha ao carregar mmdevapi.dll".to_string());
        }

        let activate_fn_ptr = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            mmdevapi,
            b"ActivateAudioInterfaceAsync\0".as_ptr(),
        );
        let activate_fn: FnActivateAudioInterfaceAsync = match activate_fn_ptr {
            Some(f) => std::mem::transmute(f),
            None => return Err("ActivateAudioInterfaceAsync não encontrada no mmdevapi.dll".to_string()),
        };

        let my_pid = windows_sys::Win32::System::Threading::GetCurrentProcessId();
        info!("🛡️ [WASAPI LOOPBACK] Configurando captura de áudio excluindo PID do Litecord ({}) da gravação...", my_pid);

        let mut act_params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: 1, // AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: my_pid,
                ProcessLoopbackMode: 1, // PROCESS_LOOPBACK_MODE_EXCLUDE_PROCESS_TREE
            },
        };

        let propvar = PROPVARIANT {
            vt: 0x0041, // VT_BLOB
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            _padding: 0,
            pBlobData: &mut act_params as *mut _ as *mut u8,
        };

        let vad_path: Vec<u16> = "VAD\\Process_Loopback\0".encode_utf16().collect();
        let event = windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null_mut());
        if event.is_null() {
            return Err("Falha ao criar evento de sincronização COM".to_string());
        }

        let client_slot: Arc<Mutex<Option<*mut c_void>>> = Arc::new(Mutex::new(None));
        let handler = Box::into_raw(Box::new(CompletionHandler {
            vtbl: &CH_VTBL,
            ref_count: AtomicU32::new(1),
            event_handle: event,
            client_slot: Arc::clone(&client_slot),
        }));

        let mut async_op: *mut IActivateAudioInterfaceAsyncOperation = std::ptr::null_mut();
        let hr = activate_fn(
            vad_path.as_ptr(),
            &IID_IAUDIOCLIENT,
            &propvar,
            handler as *mut c_void,
            &mut async_op,
        );

        if hr != 0 {
            ch_release(handler as *mut c_void);
            windows_sys::Win32::Foundation::CloseHandle(event);
            return Err(format!("ActivateAudioInterfaceAsync retornou erro hr=0x{:08x}", hr));
        }

        // Aguarda a ativação do IAudioClient (máximo 2 segundos)
        let wait_res = windows_sys::Win32::System::Threading::WaitForSingleObject(event, 2000);
        windows_sys::Win32::Foundation::CloseHandle(event);
        ch_release(handler as *mut c_void);

        if !async_op.is_null() {
            let op_vtbl = (*async_op).vtbl;
            let _ = ((*op_vtbl).Release)(async_op as *mut c_void);
        }

        if wait_res != 0 {
            return Err(format!("Timeout aguardando ativação assíncrona do WASAPI (wait=0x{:08x})", wait_res));
        }

        let client_ptr = match client_slot.lock().unwrap().take() {
            Some(p) => p as *mut IAudioClient,
            None => return Err("Ponteiro IAudioClient não foi recebido no callback de ativação".to_string()),
        };

        let client_vtbl = (*client_ptr).vtbl;
        let mut p_format: *mut WAVEFORMATEX = std::ptr::null_mut();
        let hr_fmt = ((*client_vtbl).GetMixFormat)(client_ptr as *mut c_void, &mut p_format);
        if hr_fmt != 0 || p_format.is_null() {
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("GetMixFormat falhou com hr=0x{:08x}", hr_fmt));
        }

        let fmt = &*p_format;
        let sample_rate = fmt.nSamplesPerSec;
        let channels = fmt.nChannels;
        let bits_per_sample = fmt.wBitsPerSample;

        let is_float = if fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT {
            true
        } else if fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE && fmt.cbSize >= 22 {
            let ext = &*(p_format as *const WAVEFORMATEXTENSIBLE);
            ext.SubFormat.data1 == 3 // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            false
        };

        if !is_float && bits_per_sample != 16 {
            warn!("⚠️ [WASAPI LOOPBACK] Formato de áudio não suportado: tag=0x{:04x}, bits={}", fmt.wFormatTag, bits_per_sample);
            windows_sys::Win32::System::Com::CoTaskMemFree(p_format as *mut c_void);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("Formato de áudio incompatível (bits={})", bits_per_sample));
        }

        let stream_event = windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null_mut());
        if stream_event.is_null() {
            windows_sys::Win32::System::Com::CoTaskMemFree(p_format as *mut c_void);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err("Falha ao criar evento do stream de áudio".to_string());
        }

        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        let hr_init = ((*client_vtbl).Initialize)(
            client_ptr as *mut c_void,
            0, // AUDCLNT_SHAREMODE_SHARED
            flags,
            200_000, // 20ms buffer (em unidades de 100ns)
            0,
            p_format,
            std::ptr::null(),
        );

        windows_sys::Win32::System::Com::CoTaskMemFree(p_format as *mut c_void);

        if hr_init != 0 {
            windows_sys::Win32::Foundation::CloseHandle(stream_event);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("Initialize falhou com hr=0x{:08x}", hr_init));
        }

        let hr_set = ((*client_vtbl).SetEventHandle)(client_ptr as *mut c_void, stream_event);
        if hr_set != 0 {
            windows_sys::Win32::Foundation::CloseHandle(stream_event);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("SetEventHandle falhou com hr=0x{:08x}", hr_set));
        }

        let mut capture_ptr: *mut c_void = std::ptr::null_mut();
        let hr_svc = ((*client_vtbl).GetService)(client_ptr as *mut c_void, &IID_IAUDIOCAPTURECLIENT, &mut capture_ptr);
        if hr_svc != 0 || capture_ptr.is_null() {
            windows_sys::Win32::Foundation::CloseHandle(stream_event);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("GetService IAudioCaptureClient falhou com hr=0x{:08x}", hr_svc));
        }

        let hr_start = ((*client_vtbl).Start)(client_ptr as *mut c_void);
        if hr_start != 0 {
            let capture = capture_ptr as *mut IAudioCaptureClient;
            let _ = ((*(*capture).vtbl).Release)(capture as *mut c_void);
            windows_sys::Win32::Foundation::CloseHandle(stream_event);
            let _ = ((*client_vtbl).Release)(client_ptr as *mut c_void);
            return Err(format!("Start falhou com hr=0x{:08x}", hr_start));
        }

        info!(
            "🔊 [WASAPI LOOPBACK] Captura iniciada com sucesso ({}Hz, {} canais, bits={}, float={}) com EXCLUSÃO do processo Litecord!",
            sample_rate, channels, bits_per_sample, is_float
        );

        let client_addr = client_ptr as usize;
        let capture_addr = capture_ptr as usize;
        let event_handle_addr = stream_event as usize;
        let is_running_thread = Arc::clone(&is_running);
        let pcm_buf = Arc::clone(&pcm_buffer);

        std::thread::Builder::new()
            .name("wasapi-isolated-loopback".to_string())
            .spawn(move || {
                unsafe {
                    windows_sys::Win32::System::Com::CoInitializeEx(
                        std::ptr::null_mut(),
                        windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
                    );
                    crate::cpu_profiler::set_current_thread_name("wasapi-isolated-loopback");

                    let client = client_addr as *mut IAudioClient;
                    let capture = capture_addr as *mut IAudioCaptureClient;
                    let event_handle = event_handle_addr as *mut c_void;
                    let client_vtbl = (*client).vtbl;
                    let capture_vtbl = (*capture).vtbl;

                    while is_running_thread.load(Ordering::Relaxed) {
                        let wait_res = windows_sys::Win32::System::Threading::WaitForSingleObject(event_handle, 20);
                        if wait_res != 0 && wait_res != 258 {
                            break;
                        }

                        loop {
                            let mut p_data: *mut u8 = std::ptr::null_mut();
                            let mut num_frames: u32 = 0;
                            let mut flags: u32 = 0;
                            let mut dev_pos: u64 = 0;
                            let mut qpc_pos: u64 = 0;

                            let hr_buf = ((*capture_vtbl).GetBuffer)(
                                capture as *mut c_void,
                                &mut p_data,
                                &mut num_frames,
                                &mut flags,
                                &mut dev_pos,
                                &mut qpc_pos,
                            );

                            if hr_buf != 0 || num_frames == 0 || p_data.is_null() {
                                break;
                            }

                            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0;
                            let mut buf = pcm_buf.lock().unwrap();

                            if silent {
                                let new_len = buf.len() + (num_frames as usize);
                                buf.resize(new_len, 0);
                            } else if is_float {
                                let total_samples = (num_frames as usize) * (channels as usize);
                                let f32_slice = std::slice::from_raw_parts(p_data as *const f32, total_samples);
                                let ch = channels as usize;
                                if ch == 1 {
                                    for &s in f32_slice {
                                        buf.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                                    }
                                } else {
                                    for chunk in f32_slice.chunks_exact(ch) {
                                        let mono_f = (chunk[0] + chunk[1]) * 0.5;
                                        buf.push((mono_f.clamp(-1.0, 1.0) * 32767.0) as i16);
                                    }
                                }
                            } else {
                                let total_samples = (num_frames as usize) * (channels as usize);
                                let i16_slice = std::slice::from_raw_parts(p_data as *const i16, total_samples);
                                let ch = channels as usize;
                                if ch == 1 {
                                    buf.extend_from_slice(i16_slice);
                                } else {
                                    for chunk in i16_slice.chunks_exact(ch) {
                                        let mono = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                                        buf.push(mono);
                                    }
                                }
                            }

                            let _ = ((*capture_vtbl).ReleaseBuffer)(capture as *mut c_void, num_frames);
                        }
                    }

                    info!("🔇 [WASAPI LOOPBACK] Parando captura isolada...");
                    let _ = ((*client_vtbl).Stop)(client as *mut c_void);
                    let _ = ((*capture_vtbl).Release)(capture as *mut c_void);
                    let _ = ((*client_vtbl).Release)(client as *mut c_void);
                    windows_sys::Win32::Foundation::CloseHandle(event_handle);
                    info!("🔇 [WASAPI LOOPBACK] Stream isolado finalizado com sucesso.");
                }
            })
            .ok();

        Ok(WasapiIsolatedStreamHandle {
            sample_rate,
            channels,
        })
    }
}
