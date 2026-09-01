use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::time::Instant;

type AVCodec = c_void;
type AVCodecContext = c_void;
type AVFrame = c_void;
type AVPacket = c_void;

#[repr(C)]
struct AVRational {
    num: c_int,
    den: c_int,
}

fn main() {
    println!("============================================================");
    println!("🚀 TESTE: FFMPEG HARDWARE ENCODER (h264_nvenc) ESTILO OBS/SUNSHINE");
    println!("============================================================");

    let dll_paths = [
        r"C:\Program Files\obs-studio\bin\64bit\avutil-59.dll",
        r"C:\Program Files\obs-studio\bin\64bit\swscale-8.dll",
        r"C:\Program Files\obs-studio\bin\64bit\avcodec-61.dll",
        r"C:\Users\Henrique\.scrcpy\scrcpy-win64-v3.1\avutil-59.dll",
        r"C:\Users\Henrique\.scrcpy\scrcpy-win64-v3.1\swscale-8.dll",
        r"C:\Users\Henrique\.scrcpy\scrcpy-win64-v3.1\avcodec-61.dll",
        "avutil-59.dll",
        "swscale-8.dll",
        "avcodec-61.dll",
    ];

    unsafe {
        let obs_dir = CString::new(r"C:\Program Files\obs-studio\bin\64bit").unwrap();
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryA(obs_dir.as_ptr() as *const u8);

        let scrcpy_dir = CString::new(r"C:\Users\Henrique\.scrcpy\scrcpy-win64-v3.1").unwrap();
        
        let mut avcodec_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"avcodec-61.dll\0".as_ptr());
        if avcodec_dll.is_null() {
            windows_sys::Win32::System::LibraryLoader::SetDllDirectoryA(scrcpy_dir.as_ptr() as *const u8);
            avcodec_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"avcodec-61.dll\0".as_ptr());
        }

        if avcodec_dll.is_null() {
            println!("❌ avcodec-61.dll não pôde ser carregada.");
            return;
        }
        println!("✅ avcodec-61.dll carregada com sucesso! Handle: {:p}", avcodec_dll);

        let avutil_name = CString::new("avutil-59.dll").unwrap();
        let avutil_dll = windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(avutil_name.as_ptr() as *const u8);

        type FnAvcodecFindEncoderByName = unsafe extern "C" fn(name: *const c_char) -> *mut AVCodec;
        type FnAvcodecAllocContext3 = unsafe extern "C" fn(codec: *const AVCodec) -> *mut AVCodecContext;
        type FnAvcodecFreeContext = unsafe extern "C" fn(ctx: *mut *mut AVCodecContext);
        type FnAvcodecOpen2 = unsafe extern "C" fn(ctx: *mut AVCodecContext, codec: *const AVCodec, options: *mut *mut c_void) -> c_int;
        type FnAvFrameAlloc = unsafe extern "C" fn() -> *mut AVFrame;
        type FnAvFrameFree = unsafe extern "C" fn(frame: *mut *mut AVFrame);
        type FnAvFrameGetBuffer = unsafe extern "C" fn(frame: *mut AVFrame, align: c_int) -> c_int;
        type FnAvPacketAlloc = unsafe extern "C" fn() -> *mut AVPacket;
        type FnAvPacketFree = unsafe extern "C" fn(pkt: *mut *mut AVPacket);
        type FnAvcodecSendFrame = unsafe extern "C" fn(ctx: *mut AVCodecContext, frame: *const AVFrame) -> c_int;
        type FnAvcodecReceivePacket = unsafe extern "C" fn(ctx: *mut AVCodecContext, pkt: *mut AVPacket) -> c_int;
        type FnAvPacketUnref = unsafe extern "C" fn(pkt: *mut AVPacket);
        type FnAvOptSet = unsafe extern "C" fn(obj: *mut c_void, name: *const c_char, val: *const c_char, flags: c_int) -> c_int;

        let find_encoder_fn: FnAvcodecFindEncoderByName = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_find_encoder_by_name\0".as_ptr())
        );
        let alloc_context_fn: FnAvcodecAllocContext3 = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_alloc_context3\0".as_ptr())
        );
        let open2_fn: FnAvcodecOpen2 = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_open2\0".as_ptr())
        );
        let frame_alloc_fn: FnAvFrameAlloc = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_frame_alloc\0".as_ptr())
        );
        let frame_get_buf_fn: FnAvFrameGetBuffer = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_frame_get_buffer\0".as_ptr())
        );
        let packet_alloc_fn: FnAvPacketAlloc = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"av_packet_alloc\0".as_ptr())
        );
        let send_frame_fn: FnAvcodecSendFrame = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_send_frame\0".as_ptr())
        );
        let recv_packet_fn: FnAvcodecReceivePacket = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_receive_packet\0".as_ptr())
        );
        let opt_set_fn: FnAvOptSet = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_opt_set\0".as_ptr())
        );

        type FnAvOptSetInt = unsafe extern "C" fn(obj: *mut c_void, name: *const c_char, val: i64, flags: c_int) -> c_int;
        type FnAvOptSetQ = unsafe extern "C" fn(obj: *mut c_void, name: *const c_char, q: AVRational, flags: c_int) -> c_int;

        let opt_set_int_fn: FnAvOptSetInt = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_opt_set_int\0".as_ptr())
        );
        let opt_set_q_fn: FnAvOptSetQ = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_opt_set_q\0".as_ptr())
        );

        println!("🔍 Buscando codec 'h264_nvenc'...");
        let codec_name = CString::new("h264_nvenc").unwrap();
        let codec = find_encoder_fn(codec_name.as_ptr());
        if codec.is_null() {
            println!("❌ 'h264_nvenc' não encontrado no libavcodec.");
            return;
        }
        println!("✅ Codec 'h264_nvenc' ENCONTRADO! Handle: {:p}", codec);

        let ctx = alloc_context_fn(codec);
        if ctx.is_null() {
            println!("❌ Falha ao alocar AVCodecContext.");
            return;
        }

        #[repr(C)]
        struct AVOption {
            name: *const c_char,
            help: *const c_char,
            offset: c_int,
            opt_type: c_int,
        }

        type FnAvOptFind = unsafe extern "C" fn(obj: *mut c_void, name: *const c_char, unit: *const c_char, opt_flags: c_int, search_flags: c_int) -> *const AVOption;
        let opt_find_fn: FnAvOptFind = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_opt_find\0".as_ptr())
        );

        let get_offset = |name: &[u8]| -> usize {
            let opt = opt_find_fn(ctx, name.as_ptr() as *const c_char, std::ptr::null(), 0, 0);
            if !opt.is_null() {
                (*opt).offset as usize
            } else {
                0
            }
        };

        let ctx_u8 = ctx as *mut u8;
        *(ctx_u8.add(56) as *mut i64) = 4_000_000; // bit_rate
        *(ctx_u8.add(80) as *mut u32) = 0x00080000; // flags = AV_CODEC_FLAG_LOW_DELAY
        *(ctx_u8.add(84) as *mut i32) = 1;         // time_base.num
        *(ctx_u8.add(88) as *mut i32) = 60;        // time_base.den
        *(ctx_u8.add(116) as *mut i32) = 1920;     // width
        *(ctx_u8.add(120) as *mut i32) = 1080;     // height
        *(ctx_u8.add(140) as *mut i32) = 23;       // pix_fmt = AV_PIX_FMT_NV12 (23)
        *(ctx_u8.add(148) as *mut i32) = 1;        // color_primaries = BT709
        *(ctx_u8.add(152) as *mut i32) = 1;        // color_trc = BT709
        *(ctx_u8.add(156) as *mut i32) = 1;        // colorspace = BT709
        *(ctx_u8.add(160) as *mut i32) = 2;        // color_range = PC / Full

        type FnAvDictSet = unsafe extern "C" fn(pm: *mut *mut c_void, key: *const c_char, value: *const c_char, flags: c_int) -> c_int;
        type FnAvDictFree = unsafe extern "C" fn(pm: *mut *mut c_void);

        let dict_set_fn: FnAvDictSet = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_dict_set\0".as_ptr())
        );
        let dict_free_fn: FnAvDictFree = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_dict_free\0".as_ptr())
        );

        let mut opts: *mut c_void = std::ptr::null_mut();
        dict_set_fn(&mut opts, b"preset\0".as_ptr() as *const c_char, b"p1\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"tune\0".as_ptr() as *const c_char, b"ull\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"delay\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"zerolatency\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"rc\0".as_ptr() as *const c_char, b"cbr\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"forced-idr\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"repeat-headers\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
        dict_set_fn(&mut opts, b"aud\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);

        println!("🚀 Inicializando AVCodecContext com avcodec_open2 passando AVDictionary...");
        let ret = open2_fn(ctx, codec, &mut opts as *mut *mut c_void);
        dict_free_fn(&mut opts);
        if ret < 0 {
            println!("❌ avcodec_open2 falhou com código: {}", ret);
            return;
        }
        println!("🎉 SUCESSO! NVIDIA NVENC HARDWARE ENCODER INICIALIZADO COM ZERO-LATENCY!");
        println!("🎉 SUCESSO! NVIDIA NVENC HARDWARE ENCODER INICIALIZADO NA GPU!");

        let frame = frame_alloc_fn();
        let frame_u8 = frame as *mut u8;
        *(frame_u8.add(104) as *mut i32) = 1920; // width
        *(frame_u8.add(108) as *mut i32) = 1080; // height
        *(frame_u8.add(116) as *mut i32) = 23;   // format = AV_PIX_FMT_NV12
        *(frame_u8.add(136) as *mut i64) = 0;    // pts = 0

        let buf_ret = frame_get_buf_fn(frame, 32);
        println!("   -> av_frame_get_buffer: {}", buf_ret);

        let pkt = packet_alloc_fn();

        let mut decoder = openh264::decoder::Decoder::new().unwrap();

        println!("\n⏱️ TESTANDO ENCODE CONTÍNUO DE 10 QUADROS NA RTX 3050 (1080p 60 FPS):");
        for i in 0..10 {
            *(frame_u8.add(136) as *mut i64) = i * 16666; // pts

            if i == 5 {
                println!("   [Frame #5] 👉 FORÇANDO KEYFRAME...");
                // In FFmpeg 6/7, AV_FRAME_FLAG_KEY = (1 << 0) in flags (offset 160 or 164)
                // Let's set pict_type = 1 and flags |= 1 across potential offsets
                *(frame_u8.add(120) as *mut i32) = 1; // pict_type = AV_PICTURE_TYPE_I
                *(frame_u8.add(124) as *mut i32) = 1;
                *(frame_u8.add(160) as *mut i32) = 1; // flags = AV_FRAME_FLAG_KEY
                *(frame_u8.add(164) as *mut i32) = 1;
            } else {
                *(frame_u8.add(120) as *mut i32) = 0;
                *(frame_u8.add(124) as *mut i32) = 0;
                *(frame_u8.add(160) as *mut i32) = 0;
                *(frame_u8.add(164) as *mut i32) = 0;
            }

            let t0 = Instant::now();
            let send_res = send_frame_fn(ctx, frame);
            let send_dur = t0.elapsed();

            let t1 = Instant::now();
            let recv_res = recv_packet_fn(ctx, pkt);
            let recv_dur = t1.elapsed();

            let pkt_u8 = pkt as *mut u8;
            let pkt_data = *(pkt_u8.add(24) as *mut *const u8);
            let pkt_size = *(pkt_u8.add(32) as *mut i32);

            let mut nal_types = Vec::new();
            if !pkt_data.is_null() && pkt_size > 4 {
                let bytes = std::slice::from_raw_parts(pkt_data, pkt_size as usize);
                let mut idx = 0;
                while idx + 4 <= bytes.len() {
                    if bytes[idx..idx + 4] == [0, 0, 0, 1] {
                        let nal_hdr = bytes[idx + 4];
                        let nal_type = nal_hdr & 0x1F;
                        nal_types.push(nal_type);
                        idx += 4;
                    } else if bytes[idx..idx + 3] == [0, 0, 1] {
                        let nal_hdr = bytes[idx + 3];
                        let nal_type = nal_hdr & 0x1F;
                        nal_types.push(nal_type);
                        idx += 3;
                    } else {
                        idx += 1;
                    }
                }
            }

            println!("   [Frame #{}] send={} ({:?}) | recv={} ({:?}) | H.264 NAL: {} bytes | NAL Types: {:?}",
                i, send_res, send_dur, recv_res, recv_dur, pkt_size, nal_types);

            if !pkt_data.is_null() && pkt_size > 0 {
                let bytes = std::slice::from_raw_parts(pkt_data, pkt_size as usize);
                match decoder.decode(bytes) {
                    Ok(Some(yuv)) => {
                        use openh264::formats::YUVSource;
                        println!("      🎉 DECODER OPENH264 SUCESSO! Dimensões: {:?}", yuv.dimensions());
                    }
                    Ok(None) => {
                        println!("      ⚠️ DECODER OPENH264 retornou Ok(None)");
                    }
                    Err(e) => {
                        println!("      ❌ DECODER OPENH264 ERRO: {:?}", e);
                    }
                }
            }
        }
    }
}
