use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

type AVCodec = c_void;
type AVCodecContext = c_void;
type AVFrame = c_void;
type AVPacket = c_void;

#[repr(C)]
struct AVRational {
    num: c_int,
    den: c_int,
}

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
type FnAvDictSet = unsafe extern "C" fn(pm: *mut *mut c_void, key: *const c_char, value: *const c_char, flags: c_int) -> c_int;
type FnAvDictFree = unsafe extern "C" fn(pm: *mut *mut c_void);

const MAGIC_V3: &[u8; 4] = b"LTPV";
const OP_VIDEO_CHUNK: u8 = 2;
const OP_KEYFRAME_REQ: u8 = 3;
const MAX_UDP_PAYLOAD: usize = 1200;

fn main() {
    println!("==================================================================");
    println!("🚀 LITECORD | TEST_AMD_SENDER (AMF Ultra-Fast Standalone Sender)");
    println!("==================================================================");

    let mut target_addr = "100.70.183.127:50006".to_string();
    let mut local_port = 50005;

    let args: Vec<String> = std::env::args().collect();
    for i in 1..args.len() {
        if args[i] == "--target" && i + 1 < args.len() {
            target_addr = args[i + 1].clone();
        } else if args[i] == "--port" && i + 1 < args.len() {
            local_port = args[i + 1].parse().unwrap_or(50005);
        }
    }

    println!("📡 Destino UDP: {}", target_addr);
    println!("🎧 Porta Local: {}", local_port);

    unsafe {
        let candidate_dirs = [
            r"C:\Program Files\obs-studio\bin\64bit",
            r"C:\Users\Henrique\.scrcpy\scrcpy-win64-v3.1",
            r"C:\Users\hfrei\.scrcpy\scrcpy-win64-v3.1",
            r"C:\Program Files\ldplayer9box",
            "",
        ];

        let mut avcodec_dll = std::ptr::null_mut();
        let mut avutil_dll = std::ptr::null_mut();

        for dir in candidate_dirs {
            if !dir.is_empty() {
                let c_dir = CString::new(dir).unwrap();
                windows_sys::Win32::System::LibraryLoader::SetDllDirectoryA(c_dir.as_ptr() as *const u8);
            }
            let dll_names = [b"avcodec-61.dll\0", b"avcodec-60.dll\0", b"avcodec-59.dll\0", b"avcodec-58.dll\0"];
            for dll_name in dll_names {
                avcodec_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(dll_name.as_ptr());
                if !avcodec_dll.is_null() {
                    println!("✅ Carregado {} de '{}'", String::from_utf8_lossy(dll_name), dir);
                    break;
                }
            }
            if !avcodec_dll.is_null() {
                let util_names = [b"avutil-59.dll\0", b"avutil-58.dll\0", b"avutil-57.dll\0", b"avutil-56.dll\0"];
                for util_name in util_names {
                    avutil_dll = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(util_name.as_ptr());
                    if !avutil_dll.is_null() {
                        println!("✅ Carregado {} de '{}'", String::from_utf8_lossy(util_name), dir);
                        break;
                    }
                }
                break;
            }
        }

        if avcodec_dll.is_null() || avutil_dll.is_null() {
            eprintln!("❌ Falha ao carregar FFmpeg DLLs (avcodec / avutil)");
            return;
        }

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
        let packet_unref_fn: FnAvPacketUnref = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"av_packet_unref\0".as_ptr())
        );
        let opt_set_fn: FnAvOptSet = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_opt_set\0".as_ptr())
        );
        let dict_set_fn: FnAvDictSet = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_dict_set\0".as_ptr())
        );
        let dict_free_fn: FnAvDictFree = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avutil_dll, b"av_dict_free\0".as_ptr())
        );

        let par_alloc_fn: Option<unsafe extern "C" fn() -> *mut c_void> = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_parameters_alloc\0".as_ptr())
        );
        let par_from_ctx_fn: Option<unsafe extern "C" fn(par: *mut c_void, ctx: *const c_void) -> c_int> = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_parameters_from_context\0".as_ptr())
        );
        let par_free_fn: Option<unsafe extern "C" fn(par: *mut *mut c_void)> = std::mem::transmute(
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(avcodec_dll, b"avcodec_parameters_free\0".as_ptr())
        );

        let encoder_names = ["h264_amf", "h264_nvenc", "h264_qsv"];
        let mut chosen_codec = std::ptr::null_mut();
        let mut chosen_name = "";

        for name in encoder_names {
            let c_name = CString::new(name).unwrap();
            let codec = find_encoder_fn(c_name.as_ptr());
            if !codec.is_null() {
                chosen_codec = codec;
                chosen_name = name;
                println!("🎉 GPU Encoder selecionado: {}", name);
                break;
            }
        }

        if chosen_codec.is_null() {
            eprintln!("❌ Nenhum GPU encoder encontrado!");
            return;
        }

        let width = 1280;
        let height = 720;
        let fps = 60;
        let bitrate = 6_000_000;

        let codec_ctx = alloc_context_fn(chosen_codec);
        let ctx_u8 = codec_ctx as *mut u8;
        *(ctx_u8.add(56) as *mut i64) = bitrate as i64;
        *(ctx_u8.add(80) as *mut u32) = 0x00080000; // AV_CODEC_FLAG_LOW_DELAY
        *(ctx_u8.add(84) as *mut i32) = 1;
        *(ctx_u8.add(88) as *mut i32) = fps;
        *(ctx_u8.add(116) as *mut i32) = width;
        *(ctx_u8.add(120) as *mut i32) = height;
        *(ctx_u8.add(140) as *mut i32) = 23; // AV_PIX_FMT_NV12
        *(ctx_u8.add(148) as *mut i32) = 1; // BT709
        *(ctx_u8.add(152) as *mut i32) = 1; // BT709
        *(ctx_u8.add(156) as *mut i32) = 1; // BT709
        *(ctx_u8.add(160) as *mut i32) = 2; // PC / Full range

        opt_set_fn(codec_ctx as *mut c_void, b"flags\0".as_ptr() as *const c_char, b"+low_delay\0".as_ptr() as *const c_char, 0);
        opt_set_fn(codec_ctx as *mut c_void, b"g\0".as_ptr() as *const c_char, b"30\0".as_ptr() as *const c_char, 0);
        opt_set_fn(codec_ctx as *mut c_void, b"b\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);

        let mut opts: *mut c_void = std::ptr::null_mut();
        dict_set_fn(&mut opts, b"g\0".as_ptr() as *const c_char, b"30\0".as_ptr() as *const c_char, 0);

        if chosen_name == "h264_amf" {
            dict_set_fn(&mut opts, b"usage\0".as_ptr() as *const c_char, b"transcoding\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"profile\0".as_ptr() as *const c_char, b"constrained_baseline\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"level\0".as_ptr() as *const c_char, b"3.1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"coder\0".as_ptr() as *const c_char, b"cavlc\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"quality\0".as_ptr() as *const c_char, b"speed\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"rc\0".as_ptr() as *const c_char, b"cbr\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"local_header\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"header_insertion_mode\0".as_ptr() as *const c_char, b"gop\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"cgop\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"flags\0".as_ptr() as *const c_char, b"+cgop\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"forced_idr\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"forced-idr\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"intra_refresh_type\0".as_ptr() as *const c_char, b"none\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"gops_per_idr\0".as_ptr() as *const c_char, b"1\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"header_spacing\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"filler_data\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"aud\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"max_b_frames\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
            dict_set_fn(&mut opts, b"b_frame_delta_qp\0".as_ptr() as *const c_char, b"0\0".as_ptr() as *const c_char, 0);
        }

        let open_res = open2_fn(codec_ctx, chosen_codec, &mut opts);
        dict_free_fn(&mut opts);
        if open_res < 0 {
            eprintln!("❌ Falha ao abrir encoder {} (res={})", chosen_name, open_res);
            return;
        }

        let mut captured_sps_pps: Option<Vec<u8>> = None;
        if let (Some(alloc_par), Some(from_ctx), Some(free_par)) = (par_alloc_fn, par_from_ctx_fn, par_free_fn) {
            let par = alloc_par();
            if !par.is_null() {
                let res = from_ctx(par, codec_ctx as *const c_void);
                let par_u8 = par as *mut u8;
                let ext_ptr = *(par_u8.add(16) as *mut *const u8);
                let ext_sz = *(par_u8.add(24) as *mut i32);
                println!("🎯 [PAR FROM CTX] res={} ext_ptr={:p} ext_sz={}", res, ext_ptr, ext_sz);
                if !ext_ptr.is_null() && ext_sz > 0 {
                    let ed = std::slice::from_raw_parts(ext_ptr, ext_sz as usize);
                    println!("🎯 [EXTRADATA VIA PAR] len={} bytes: {:02X?}", ext_sz, ed);
                    captured_sps_pps = Some(ed.to_vec());
                }
                let mut p = par;
                free_par(&mut p);
            }
        }

        println!("✅ Encoder {} inicializado com sucesso!", chosen_name);

        let frame = frame_alloc_fn();
        let frame_u8 = frame as *mut u8;
        *(frame_u8.add(104) as *mut i32) = width;
        *(frame_u8.add(108) as *mut i32) = height;
        *(frame_u8.add(116) as *mut i32) = 23; // NV12
        let buf_res = frame_get_buf_fn(frame, 32);
        println!("📦 frame_get_buffer resultado: {}", buf_res);
        if buf_res < 0 {
            eprintln!("❌ Falha ao alocar buffers do AVFrame");
            return;
        }

        let packet = packet_alloc_fn();

        let socket = UdpSocket::bind(format!("0.0.0.0:{}", local_port))
            .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
            .expect("Falha ao abrir socket UDP");
        println!("🎧 Socket vinculado na porta: {:?}", socket.local_addr().unwrap().port());
        socket.set_nonblocking(true).expect("Falha ao setar nonblocking");

        let mut force_pli = false;

        let y_plane_size = (width * height) as usize;
        let uv_plane_size = (width * height / 2) as usize;

        let mut y_plane = vec![128u8; y_plane_size];
        let uv_plane = vec![128u8; uv_plane_size];

        let sender_uid = 995123987032055918u64;
        let mut seq = 0u32;
        let mut last_idr = Instant::now() - Duration::from_secs(1);

        println!("🎬 Transmissão ao vivo iniciada para {} a 60 FPS...", target_addr);

        let frame_interval = Duration::from_micros(16666);
        let mut next_frame_time = Instant::now();

        let mut out_buffer = Vec::with_capacity(256 * 1024);

        let data_ptrs = frame_u8 as *mut *mut u8;
        let linesize_ptrs = frame_u8.add(64) as *mut i32;

        let y_ptr = *data_ptrs;
        let uv_ptr = *data_ptrs.add(1);
        let y_linesize = *linesize_ptrs as usize;
        let uv_linesize = *linesize_ptrs.add(1) as usize;

        println!("📊 Buffer info: y_ptr={:p}, uv_ptr={:p}, y_linesize={}, uv_linesize={}", y_ptr, uv_ptr, y_linesize, uv_linesize);

        if y_ptr.is_null() || uv_ptr.is_null() || y_linesize == 0 || uv_linesize == 0 {
            eprintln!("❌ Ponteiros ou strides do AVFrame inválidos!");
            return;
        }

        loop {
            // Checa PLI requests recebidos do receptor (offset 4 ou offset 8)
            let mut rx_buf = [0u8; 1500];
            while let Ok((n, _src)) = socket.recv_from(&mut rx_buf) {
                if n >= 5 && &rx_buf[..4] == MAGIC_V3 && (rx_buf[4] == OP_KEYFRAME_REQ || (n >= 9 && rx_buf[8] == OP_KEYFRAME_REQ)) {
                    println!("🔄 [PLI RX] Pedido de Keyframe recebido do receptor! Forçando IDR...");
                    force_pli = true;
                }
            }

            let is_key_req = force_pli || last_idr.elapsed() >= Duration::from_millis(1000);
            if is_key_req {
                last_idr = Instant::now();
                force_pli = false;
            }

            // Anima padrão YUV de teste com gradiente em movimento
            let offset = (seq * 4) as u8;
            for (idx, b) in y_plane.iter_mut().enumerate() {
                let x = (idx % width as usize) as u8;
                let y = (idx / width as usize) as u8;
                *b = x.wrapping_add(y).wrapping_add(offset);
            }

            // Copia para buffers do AVFrame
            for row in 0..height as usize {
                let src_offset = row * width as usize;
                let dst_offset = row * y_linesize;
                std::ptr::copy_nonoverlapping(y_plane.as_ptr().add(src_offset), y_ptr.add(dst_offset), width as usize);
            }

            for row in 0..(height / 2) as usize {
                let src_offset = row * width as usize;
                let dst_offset = row * uv_linesize;
                std::ptr::copy_nonoverlapping(uv_plane.as_ptr().add(src_offset), uv_ptr.add(dst_offset), width as usize);
            }

            let pts_val = (seq as i64) * 16666;
            *(frame_u8.add(132) as *mut i64) = pts_val;
            *(frame_u8.add(136) as *mut i64) = pts_val;

            if is_key_req {
                *(frame_u8.add(116) as *mut i32) = 23; // NV12
                *(frame_u8.add(120) as *mut i32) = 1;  // FFmpeg 6/7 pict_type = AV_PICTURE_TYPE_I
                *(frame_u8.add(124) as *mut i32) = 1;  // FFmpeg 5 pict_type = AV_PICTURE_TYPE_I
                *(frame_u8.add(380) as *mut i32) |= 2; // FFmpeg 6/7 flags |= AV_FRAME_FLAG_KEY
                *(frame_u8.add(384) as *mut i32) |= 2; // FFmpeg 5 flags |= AV_FRAME_FLAG_KEY
            } else {
                *(frame_u8.add(116) as *mut i32) = 23; // NV12
                *(frame_u8.add(120) as *mut i32) = 0;
                *(frame_u8.add(124) as *mut i32) = 0;
                *(frame_u8.add(380) as *mut i32) &= !2;
                *(frame_u8.add(384) as *mut i32) &= !2;
            }

            let send_res = send_frame_fn(codec_ctx, frame);
            if seq < 5 || seq % 60 == 0 {
                println!("🎬 [SEND FRAME #{}] res={} (KeyReq={})", seq, send_res, is_key_req);
            }
            if send_res >= 0 {
                out_buffer.clear();
                loop {
                    let recv_res = recv_packet_fn(codec_ctx, packet);
                    if seq < 5 {
                        println!("📦 [RECV PACKET] res={}", recv_res);
                    }
                    if recv_res < 0 {
                        break;
                    }
                    let pkt_u8 = packet as *mut u8;
                    let pkt_data = *(pkt_u8.add(24) as *mut *const u8);
                    let pkt_size = *(pkt_u8.add(32) as *mut i32) as usize;
                    if seq < 5 {
                        println!("📦 [PACKET INFO] pkt_data={:p}, pkt_size={}", pkt_data, pkt_size);
                    }
                    if !pkt_data.is_null() && pkt_size > 0 {
                        let chunk = std::slice::from_raw_parts(pkt_data, pkt_size);
                        out_buffer.extend_from_slice(chunk);
                    }
                    packet_unref_fn(packet);
                }

                if !out_buffer.is_empty() {
                    let mut nals = Vec::new();
                    for w in out_buffer.windows(5) {
                        if w[..4] == [0, 0, 0, 1] {
                            nals.push(w[4] & 0x1F);
                        }
                    }

                    let mut final_payload = Vec::new();
                    if nals.contains(&5) && !nals.contains(&7) {
                        if let Some(ref sps) = captured_sps_pps {
                            final_payload.extend_from_slice(sps);
                        }
                    }
                    final_payload.extend_from_slice(&out_buffer);

                    // Fragmenta e envia pacotes UDP no formato oficial do Litecord (37 bytes header)
                    let payload = if final_payload.is_empty() { &out_buffer } else { &final_payload };
                    let total_chunks = ((payload.len() + MAX_UDP_PAYLOAD - 1) / MAX_UDP_PAYLOAD).max(1);
                    for (chunk_idx, slice) in payload.chunks(MAX_UDP_PAYLOAD).enumerate() {
                        let mut udp_pkt = Vec::with_capacity(37 + slice.len());
                        udp_pkt.extend_from_slice(MAGIC_V3); // [0..4]
                        udp_pkt.extend_from_slice(&seq.to_be_bytes()); // [4..8]
                        udp_pkt.push(OP_VIDEO_CHUNK); // [8] OP=2
                        udp_pkt.extend_from_slice(&1310372456904654931u64.to_be_bytes()); // [9..17] channel_id
                        udp_pkt.extend_from_slice(&sender_uid.to_be_bytes()); // [17..25] sender_uid
                        udp_pkt.extend_from_slice(&seq.to_be_bytes()); // [25..29] frame_seq
                        udp_pkt.extend_from_slice(&(seq * 16).to_be_bytes()); // [29..33] timestamp
                        udp_pkt.extend_from_slice(&(total_chunks as u16).to_be_bytes()); // [33..35] total_chunks
                        udp_pkt.extend_from_slice(&(chunk_idx as u16).to_be_bytes()); // [35..37] chunk_idx
                        udp_pkt.extend_from_slice(slice); // [37..] payload

                        let _ = socket.send_to(&udp_pkt, &target_addr);
                    }
                }
            }

            seq = seq.wrapping_add(1);
            next_frame_time += frame_interval;
            let now = Instant::now();
            if next_frame_time > now {
                std::thread::sleep(next_frame_time - now);
            } else {
                next_frame_time = now;
            }
        }
    }
}
