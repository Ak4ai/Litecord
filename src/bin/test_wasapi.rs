#![cfg(windows)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

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
    data1: 0x1cb9ad4c,
    data2: 0xdbfa,
    data3: 0x4c32,
    data4: [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2],
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

pub const IID_IAGILEOBJECT: GUID = GUID {
    data1: 0x94ea2b94,
    data2: 0xe9cc,
    data3: 0x49e0,
    data4: [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90],
};

pub const IID_IMARSHAL: GUID = GUID {
    data1: 0x00000003,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
#[derive(Debug)]
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
pub struct CompletionHandler {
    pub vtbl: &'static IActivateAudioInterfaceCompletionHandlerVtbl,
    pub ref_count: AtomicU32,
    pub event_handle: *mut c_void,
    pub client_slot: Arc<Mutex<Option<*mut c_void>>>,
    pub ftm: *mut c_void,
}

unsafe extern "system" fn ch_query_interface(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return -2147467261;
    }
    let guid = *riid;
    let handler = &*(this as *mut CompletionHandler);
    if guid_eq(&guid, &IID_IUNKNOWN)
        || guid_eq(&guid, &IID_IACTIVATEAUDIOINTERFACECOMPLETIONHANDLER)
        || guid_eq(&guid, &IID_IAGILEOBJECT)
    {
        *ppv = this;
        ch_add_ref(this);
        0
    } else if guid_eq(&guid, &IID_IMARSHAL) && !handler.ftm.is_null() {
        let ftm_unk = *(handler.ftm as *mut *mut IActivateAudioInterfaceAsyncOperationVtbl);
        ((*ftm_unk).QueryInterface)(handler.ftm, riid, ppv)
    } else {
        *ppv = std::ptr::null_mut();
        -2147467262
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
        if !handler.ftm.is_null() {
            let ftm_unk = *(handler.ftm as *mut *mut IActivateAudioInterfaceAsyncOperationVtbl);
            ((*ftm_unk).Release)(handler.ftm);
        }
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
        println!("Activated result hr=0x{:08x}, hr_res=0x{:08x}, unk={:?}", hr, hr_res, unk);
        if hr == 0 && hr_res == 0 && !unk.is_null() {
            *handler.client_slot.lock().unwrap() = Some(unk);
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

fn main() {
    println!("=== WASAPI PROCESS LOOPBACK TEST ===");
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null_mut(), windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32);
        let mmdevapi = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"mmdevapi.dll\0".as_ptr());
        if mmdevapi.is_null() {
            println!("Failed to load mmdevapi.dll");
            return;
        }
        let activate_fn_ptr = windows_sys::Win32::System::LibraryLoader::GetProcAddress(mmdevapi, b"ActivateAudioInterfaceAsync\0".as_ptr());
        if activate_fn_ptr.is_none() {
            println!("ActivateAudioInterfaceAsync not found");
            return;
        }
        let activate_fn: FnActivateAudioInterfaceAsync = std::mem::transmute(activate_fn_ptr);

        let my_pid = windows_sys::Win32::System::Threading::GetCurrentProcessId();
        println!("Current Process ID: {}", my_pid);

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
        let client_slot: Arc<Mutex<Option<*mut c_void>>> = Arc::new(Mutex::new(None));

        let mut ftm: *mut c_void = std::ptr::null_mut();
        windows_sys::Win32::System::Com::CoCreateFreeThreadedMarshaler(std::ptr::null_mut(), &mut ftm);

        let handler = Box::into_raw(Box::new(CompletionHandler {
            vtbl: &CH_VTBL,
            ref_count: AtomicU32::new(1),
            event_handle: event,
            client_slot: Arc::clone(&client_slot),
            ftm,
        }));

        let mut async_op: *mut IActivateAudioInterfaceAsyncOperation = std::ptr::null_mut();
        let hr = activate_fn(
            vad_path.as_ptr(),
            &IID_IAUDIOCLIENT,
            &propvar,
            handler as *mut c_void,
            &mut async_op,
        );
        println!("ActivateAudioInterfaceAsync returned hr=0x{:08x}", hr);

        if hr == 0 {
            let wait_res = windows_sys::Win32::System::Threading::WaitForSingleObject(event, 2000);
            println!("WaitForSingleObject returned 0x{:08x}", wait_res);
            let client_ptr_opt = client_slot.lock().unwrap().take();
            if let Some(client_ptr) = client_ptr_opt {
                let client = client_ptr as *mut IAudioClient;
                println!("Successfully received IAudioClient pointer: {:?}", client);
                let client_vtbl = (*client).vtbl;
                let mut p_format: *mut WAVEFORMATEX = std::ptr::null_mut();
                let hr_fmt = ((*client_vtbl).GetMixFormat)(client as *mut c_void, &mut p_format);
                println!("GetMixFormat hr=0x{:08x}", hr_fmt);
                if hr_fmt == 0 && !p_format.is_null() {
                    let fmt = &*p_format;
                    println!("Mix format: rate={} channels={} bits={}", fmt.nSamplesPerSec, fmt.nChannels, fmt.wBitsPerSample);
                    let stream_event = windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null_mut());
                    let flags = 0x00020000 | 0x00040000; // LOOPBACK | EVENTCALLBACK
                    let hr_init = ((*client_vtbl).Initialize)(
                        client as *mut c_void,
                        0, // SHARED
                        flags,
                        200_000, // 20ms buffer
                        0,
                        p_format,
                        std::ptr::null(),
                    );
                    println!("Initialize hr=0x{:08x}", hr_init);
                    if hr_init == 0 {
                        let hr_set = ((*client_vtbl).SetEventHandle)(client as *mut c_void, stream_event);
                        println!("SetEventHandle hr=0x{:08x}", hr_set);
                        let mut capture_ptr: *mut c_void = std::ptr::null_mut();
                        let hr_svc = ((*client_vtbl).GetService)(client as *mut c_void, &IID_IAUDIOCAPTURECLIENT, &mut capture_ptr);
                        println!("GetService IAudioCaptureClient hr=0x{:08x}", hr_svc);
                        if hr_svc == 0 && !capture_ptr.is_null() {
                            let capture = capture_ptr as *mut IAudioCaptureClient;
                            let capture_vtbl = (*capture).vtbl;
                            let hr_start = ((*client_vtbl).Start)(client as *mut c_void);
                            println!("Start hr=0x{:08x}", hr_start);
                            println!("Audio capture started! Capturing 5 packets...");
                            for i in 0..5 {
                                windows_sys::Win32::System::Threading::WaitForSingleObject(stream_event, 200);
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
                                if hr_buf == 0 && num_frames > 0 {
                                    println!("Packet {}: {} frames read, silent={}", i, num_frames, (flags & 0x2) != 0);
                                    let _ = ((*capture_vtbl).ReleaseBuffer)(capture as *mut c_void, num_frames);
                                }
                            }
                            let _ = ((*client_vtbl).Stop)(client as *mut c_void);
                            println!("SUCCESS! Audio capture completed cleanly!");
                            let _ = ((*capture_vtbl).Release)(capture as *mut c_void);
                        }
                        windows_sys::Win32::Foundation::CloseHandle(stream_event);
                    }
                    windows_sys::Win32::System::Com::CoTaskMemFree(p_format as *mut c_void);
                }
                let _ = ((*client_vtbl).Release)(client as *mut c_void);
            } else {
                println!("Failed to receive IAudioClient pointer");
            }
        }
        if !async_op.is_null() {
            let op_vtbl = (*async_op).vtbl;
            let _ = ((*op_vtbl).Release)(async_op as *mut c_void);
        }
        ch_release(handler as *mut c_void);
        windows_sys::Win32::Foundation::CloseHandle(event);
    }
}
