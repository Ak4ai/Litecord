#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use log::{info, warn};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::types::*;
use super::crypto::*;

static STREAM_AUDIO_PLAYBACK_INIT: OnceLock<()> = OnceLock::new();
static STREAM_AUDIO_QUEUE: OnceLock<Arc<Mutex<VecDeque<f32>>>> = OnceLock::new();
static STREAM_VOLUMES: OnceLock<Arc<Mutex<HashMap<u64, f32>>>> = OnceLock::new();

pub fn get_stream_audio_queue() -> Arc<Mutex<VecDeque<f32>>> {
    STREAM_AUDIO_QUEUE.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(48000 * 2)))).clone()
}

pub fn clear_stream_audio_queue() {
    if let Ok(mut q) = get_stream_audio_queue().lock() {
        q.clear();
    }
}

pub fn get_stream_volumes() -> Arc<Mutex<HashMap<u64, f32>>> {
    STREAM_VOLUMES.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

pub fn set_stream_volume(uid: u64, vol: f32) {
    if let Ok(mut map) = get_stream_volumes().lock() {
        map.insert(uid, vol.clamp(0.0, 2.0));
    }
}

pub fn get_stream_volume(uid: u64) -> f32 {
    if let Ok(map) = get_stream_volumes().lock() {
        map.get(&uid).copied().unwrap_or(1.0)
    } else {
        1.0
    }
}

pub fn ensure_stream_audio_playback_started() {
    STREAM_AUDIO_PLAYBACK_INIT.get_or_init(|| {
        std::thread::Builder::new()
            .name("stream-audio-playback".to_string())
            .spawn(|| {
                #[cfg(windows)]
                unsafe {
                    windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null_mut(), windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32);
                    windows_sys::Win32::Media::timeBeginPeriod(1);
                }

                let host = cpal::default_host();
                let queue = get_stream_audio_queue();

                info!("🔊 [STREAM PLAYBACK] Inicializando subsistema de saída de áudio para stream...");

                loop {
                    if crate::gateway::CURRENT_VOICE_SESSION_ID.load(Ordering::Relaxed) != 0 {
                        // Voice Gateway Master Output Engine is actively running and mixing stream audio
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    let selected_dev_name = if let Ok(guard) = crate::gateway::get_selected_output_device_store().lock() {
                        guard.clone()
                    } else {
                        String::new()
                    };

                    let mut devices_to_try: Vec<cpal::Device> = Vec::new();
                    if !selected_dev_name.is_empty() {
                        if let Ok(devs) = host.output_devices() {
                            for d in devs {
                                if d.name().map_or(false, |n| n == selected_dev_name) {
                                    devices_to_try.push(d);
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(def) = host.default_output_device() {
                        let def_name = def.name().unwrap_or_default();
                        if !def_name.contains("Litecord") && !def_name.contains("Virtual") && !def_name.contains("Null") && !def_name.contains("Steam") {
                            if !devices_to_try.iter().any(|d| d.name().ok() == def.name().ok()) {
                                devices_to_try.push(def);
                            }
                        }
                    }
                    if let Ok(devs) = host.output_devices() {
                        for d in devs {
                            let name = d.name().unwrap_or_default();
                            if name.contains("Steam") || name.contains("Virtual") || name.contains("Null") || name.contains("Litecord") {
                                continue;
                            }
                            if !devices_to_try.iter().any(|existing| existing.name().ok() == d.name().ok()) {
                                devices_to_try.push(d);
                            }
                        }
                    }

                    if devices_to_try.is_empty() {
                        warn!("⚠️ [STREAM PLAYBACK] Nenhum dispositivo de saída encontrado!");
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }

                    let mut active_stream = None;

                    for dev in devices_to_try {
                        let dev_name = dev.name().unwrap_or_else(|_| "Dispositivo Desconhecido".to_string());
                        let config = match dev.default_output_config() {
                            Ok(c) => c,
                            Err(e) => {
                                warn!("⚠️ [STREAM PLAYBACK] Falha ao obter default_output_config de '{}': {:?}", dev_name, e);
                                continue;
                            }
                        };
                        let channels = config.channels() as usize;
                        let sample_rate = config.sample_rate().0;

                        let last_err_time = Arc::new(Mutex::new(Option::<Instant>::None));
                        let last_err_c = Arc::clone(&last_err_time);
                        let dev_name_err = dev_name.clone();
                        let err_fn = move |err| {
                            if let Ok(mut guard) = last_err_c.lock() {
                                if guard.map_or(true, |t| t.elapsed() >= Duration::from_secs(5)) {
                                    *guard = Some(Instant::now());
                                    warn!("Erro no playback de áudio do stream ({}): {}", dev_name_err, err);
                                }
                            }
                        };

                        let sample_format = config.sample_format();
                        let stream_res = match sample_format {
                            cpal::SampleFormat::F32 => {
                                let q = Arc::clone(&queue);
                                dev.build_output_stream(
                                    &config.into(),
                                    move |data: &mut [f32], _| {
                                        if crate::gateway::CURRENT_VOICE_SESSION_ID.load(Ordering::Relaxed) != 0 || crate::gateway::get_my_voice_channel_id() != 0 {
                                            data.fill(0.0);
                                            return;
                                        }
                                        let mut queue_guard = q.lock().unwrap_or_else(|e| e.into_inner());
                                        let q_len = queue_guard.len();
                                        if q_len > 4800 {
                                            let excess = q_len - 2400;
                                            queue_guard.drain(0..excess);
                                            let fade_len = 32.min(queue_guard.len());
                                            for i in 0..fade_len {
                                                let factor = i as f32 / fade_len as f32;
                                                queue_guard[i] *= factor;
                                            }
                                        }
                                        for chunk in data.chunks_mut(channels) {
                                            let sample = queue_guard.pop_front().unwrap_or(0.0);
                                            for channel_sample in chunk.iter_mut() {
                                                *channel_sample = sample;
                                            }
                                        }
                                    },
                                    err_fn,
                                    None,
                                )
                            }
                            cpal::SampleFormat::I16 => {
                                let q = Arc::clone(&queue);
                                dev.build_output_stream(
                                    &config.into(),
                                    move |data: &mut [i16], _| {
                                        if crate::gateway::CURRENT_VOICE_SESSION_ID.load(Ordering::Relaxed) != 0 || crate::gateway::get_my_voice_channel_id() != 0 {
                                            data.fill(0);
                                            return;
                                        }
                                        let mut queue_guard = q.lock().unwrap_or_else(|e| e.into_inner());
                                        let q_len = queue_guard.len();
                                        if q_len > 4800 {
                                            let excess = q_len - 2400;
                                            queue_guard.drain(0..excess);
                                            let fade_len = 32.min(queue_guard.len());
                                            for i in 0..fade_len {
                                                let factor = i as f32 / fade_len as f32;
                                                queue_guard[i] *= factor;
                                            }
                                        }
                                        for chunk in data.chunks_mut(channels) {
                                            let sample_f = queue_guard.pop_front().unwrap_or(0.0);
                                            let sample_i = (sample_f.clamp(-1.0, 1.0) * 32767.0) as i16;
                                            for channel_sample in chunk.iter_mut() {
                                                *channel_sample = sample_i;
                                            }
                                        }
                                    },
                                    err_fn,
                                    None,
                                )
                            }
                            _ => continue,
                        };

                        if let Ok(stream) = stream_res {
                            if stream.play().is_ok() {
                                info!("🔊 [STREAM PLAYBACK] Saída de áudio ATIVA no dispositivo '{}' ({}Hz, {} canais, formato={:?})!", dev_name, sample_rate, channels, sample_format);
                                active_stream = Some(stream);
                                break;
                            }
                        }
                    }

                    if let Some(_stream) = active_stream {
                        loop {
                            std::thread::sleep(Duration::from_secs(3600));
                        }
                    } else {
                        warn!("⚠️ [STREAM PLAYBACK] Falha ao iniciar playback em todos os dispositivos. Tentando novamente em 3 segundos...");
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
            })
            .expect("Falha ao iniciar thread de playback de áudio do stream");
    });
}

#[cfg(target_os = "linux")]
struct LinuxLoopbackSinkGuard {
    original_sink: String,
    mod_null: Option<String>,
    mod_loopback: Option<String>,
    stop_flag: Arc<AtomicBool>,
    recheck_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl LinuxLoopbackSinkGuard {
    fn empty() -> Self {
        Self {
            original_sink: String::new(),
            mod_null: None,
            mod_loopback: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            recheck_thread: None,
        }
    }

    fn unload_previous_litecord_modules() {
        if let Ok(output) = std::process::Command::new("pactl").args(["list", "modules", "short"]).output() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                for line in text.lines() {
                    if line.contains("LitecordDesktop") {
                        if let Some(mod_id) = line.split_whitespace().next() {
                            let _ = std::process::Command::new("pactl").args(["unload-module", mod_id]).status();
                        }
                    }
                }
            }
        }
    }

    fn move_litecord_sink_inputs(target_sink: &str) {
        let pid_str = std::process::id().to_string();
        if let Ok(output) = std::process::Command::new("pactl").args(["list", "sink-inputs"]).output() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                let mut current_id: Option<String> = None;
                let mut is_litecord = false;

                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("Sink Input #") || trimmed.starts_with("Sink-Input #") || (trimmed.contains('#') && trimmed.to_lowercase().contains("sink-input")) {
                        if let (Some(id), true) = (current_id.take(), is_litecord) {
                            let _ = std::process::Command::new("pactl").args(["move-sink-input", &id, target_sink]).status();
                        }
                        is_litecord = false;
                        if let Some(pos) = trimmed.find('#') {
                            let id_str: String = trimmed[pos + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
                            if !id_str.is_empty() {
                                current_id = Some(id_str);
                            }
                        }
                    } else {
                        let lower = trimmed.to_lowercase();
                        if lower.contains("litecord") || trimmed.contains(&pid_str) {
                            is_litecord = true;
                        }
                    }
                }
                if let (Some(id), true) = (current_id, is_litecord) {
                    let _ = std::process::Command::new("pactl").args(["move-sink-input", &id, target_sink]).status();
                }
            }
        }
    }

    fn setup() -> Self {
        // Limpa módulos anteriores que possam ter sobrado de uma execução anterior
        Self::unload_previous_litecord_modules();

        let orig = std::process::Command::new("pactl")
            .arg("get-default-sink")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if orig.is_empty() || orig.contains("Litecord") {
            return Self {
                original_sink: orig,
                mod_null: None,
                mod_loopback: None,
                stop_flag: Arc::new(AtomicBool::new(false)),
                recheck_thread: None,
            };
        }

        let mod_null = std::process::Command::new("pactl")
            .args(["load-module", "module-null-sink", "sink_name=LitecordDesktopSink", "sink_properties=device.description=Litecord_Desktop_Audio"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string());

        let mod_loopback = if let Some(ref _mn) = mod_null {
            std::process::Command::new("pactl")
                .args(["load-module", "module-loopback", "source=LitecordDesktopSink.monitor", &format!("sink={}", orig), "latency_msec=10"])
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|s| s.trim().to_string())
        } else {
            None
        };

        if mod_null.is_some() && mod_loopback.is_some() {
            let _ = std::process::Command::new("pactl")
                .args(["set-default-sink", "LitecordDesktopSink"])
                .status();
            info!("🛡️ [LOOPBACK TX] Sink virtual isolado 'LitecordDesktopSink' criado (áudio do Litecord excluído da transmissão de tela)!");

            // Move imediatamente qualquer stream já aberta do Litecord de volta para o hardware físico
            Self::move_litecord_sink_inputs(&orig);
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let orig_for_thread = orig.clone();
        let stop_for_thread = Arc::clone(&stop_flag);

        let recheck_thread = if mod_null.is_some() && mod_loopback.is_some() && !orig.is_empty() {
            Some(std::thread::spawn(move || {
                while !stop_for_thread.load(Ordering::Relaxed) {
                    Self::move_litecord_sink_inputs(&orig_for_thread);
                    std::thread::sleep(Duration::from_millis(500));
                }
            }))
        } else {
            None
        };

        Self {
            original_sink: orig,
            mod_null,
            mod_loopback,
            stop_flag,
            recheck_thread,
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxLoopbackSinkGuard {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(t) = self.recheck_thread.take() {
            let _ = t.join();
        }
        if !self.original_sink.is_empty() && !self.original_sink.contains("Litecord") {
            let _ = std::process::Command::new("pactl")
                .args(["set-default-sink", &self.original_sink])
                .status();
            info!("🛡️ [LOOPBACK TX] Sink padrão restaurado para '{}'", self.original_sink);
        }
        if let Some(ref m) = self.mod_loopback {
            let _ = std::process::Command::new("pactl")
                .args(["unload-module", m])
                .status();
        }
        if let Some(ref m) = self.mod_null {
            let _ = std::process::Command::new("pactl")
                .args(["unload-module", m])
                .status();
        }
    }
}

pub fn start_audio_loopback_tx(
    is_running: Arc<AtomicBool>,
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    peers_store: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    std::thread::Builder::new()
        .name("audio-loopback-tx".to_string())
        .spawn(move || {
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null_mut(), windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32);
                windows_sys::Win32::Media::timeBeginPeriod(1);
            }
            crate::cpu_profiler::set_current_thread_name("audio-loopback-tx");

            let mut seq: u32 = 0;
            let pcm_buffer: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(4800)));
            let pcm_buffer_cb = Arc::clone(&pcm_buffer);

            let mut active_sample_rate: u32 = 48000;
            let mut target_chunk_samples: usize = 480;

            #[cfg(target_os = "linux")]
            let loopback_backend = crate::video_settings::get_audio_loopback_backend();
            #[cfg(target_os = "linux")]
            let try_pulse = loopback_backend == "auto" || loopback_backend == "pulsesrc";

            #[cfg(target_os = "linux")]
            let _sink_guard = if try_pulse {
                LinuxLoopbackSinkGuard::setup()
            } else {
                LinuxLoopbackSinkGuard::empty()
            };

            #[cfg(target_os = "linux")]
            let mut gst_child: Option<std::process::Child> = None;

            #[cfg(target_os = "linux")]
            if try_pulse {
                let monitor_device = if _sink_guard.mod_null.is_some() {
                    "LitecordDesktopSink.monitor"
                } else {
                    "@DEFAULT_SINK@.monitor"
                };

                info!("🎙️ [LOOPBACK TX] Inicializando captura de áudio da transmissão (Linux PulseAudio/PipeWire no dispositivo '{}')...", monitor_device);
                let mut cmd = std::process::Command::new("gst-launch-1.0");
                cmd.env_remove("PIPEWIRE_NODE");
                cmd.args([
                    "-q",
                    "pulsesrc",
                    &format!("device={}", monitor_device),
                    "buffer-time=20000",
                    "latency-time=10000",
                    "!",
                    "audioconvert",
                    "!",
                    "audio/x-raw,format=S16LE,rate=48000,channels=1",
                    "!",
                    "fdsink",
                    "fd=1",
                ]);
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::null());

                match cmd.spawn() {
                    Ok(mut child) => {
                        if let Some(mut stdout) = child.stdout.take() {
                            active_sample_rate = 48000;
                            target_chunk_samples = 480; // 10ms @ 48kHz
                            let pcm_buf = Arc::clone(&pcm_buffer_cb);
                            let is_running_reader = Arc::clone(&is_running);

                            std::thread::Builder::new()
                                .name("gst-audio-loopback-reader".to_string())
                                .spawn(move || {
                                    use std::io::Read;
                                    let mut raw = [0u8; 960]; // 480 samples * 2 bytes = 10ms
                                    while is_running_reader.load(Ordering::Relaxed) {
                                        match stdout.read_exact(&mut raw) {
                                            Ok(_) => {
                                                let mut buf = pcm_buf.lock().unwrap_or_else(|e| e.into_inner());
                                                for chunk in raw.chunks_exact(2) {
                                                    let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                                                    buf.push(s);
                                                }
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                })
                                .ok();

                            info!("🔊 [LOOPBACK TX] Captura de áudio do sistema PipeWire/PulseAudio iniciada com sucesso (48000Hz, 1 canal, chunk=480 amostras/10ms)!");
                            gst_child = Some(child);
                        } else {
                            warn!("⚠️ [LOOPBACK TX] Falha ao capturar stdout do GStreamer pulsesrc!");
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [LOOPBACK TX] Falha ao iniciar gst-launch-1.0 pulsesrc: {:?}", e);
                    }
                }
            }

            #[cfg(target_os = "windows")]
            let mut active_stream = None;
            #[cfg(target_os = "windows")]
            let mut wasapi_isolated_handle = None;

            #[cfg(target_os = "windows")]
            {
                let loopback_backend = crate::video_settings::get_audio_loopback_backend();
                let try_wasapi_isolated = loopback_backend == "auto" || loopback_backend == "wasapi_isolated";

                if try_wasapi_isolated {
                    info!("🎙️ [LOOPBACK TX] Inicializando captura de áudio nativa isolada (WASAPI Process Loopback)...");
                    match crate::wasapi_loopback::start_wasapi_isolated_loopback(
                        Arc::clone(&is_running),
                        Arc::clone(&pcm_buffer_cb),
                    ) {
                        Ok(handle) => {
                            active_sample_rate = handle.sample_rate;
                            target_chunk_samples = ((handle.sample_rate as usize) * 10) / 1000;
                            info!("🔊 [LOOPBACK TX] Captura de áudio ISOLADA nativa WASAPI ATIVA (excluindo Litecord) ({}Hz, {} canais, chunk={} amostras/10ms)!", handle.sample_rate, handle.channels, target_chunk_samples);
                            wasapi_isolated_handle = Some(handle);
                        }
                        Err(e) => {
                            warn!("⚠️ [LOOPBACK TX] WASAPI Process Loopback isolado indisponível ({}), utilizando fallback CPAL...", e);
                        }
                    }
                }

                if wasapi_isolated_handle.is_none() {
                    let host = cpal::default_host();

                    let selected_dev_name = if let Ok(guard) = crate::gateway::get_selected_output_device_store().lock() {
                        guard.clone()
                    } else {
                        String::new()
                    };

                    let devices_to_try: Vec<cpal::Device> = {
                        let mut list = Vec::new();
                        if !selected_dev_name.is_empty() {
                            if let Ok(devs) = host.output_devices() {
                                for d in devs {
                                    if d.name().map_or(false, |n| n == selected_dev_name) {
                                        list.push(d);
                                        break;
                                    }
                                }
                            }
                        }
                        if let Some(def) = host.default_output_device() {
                            if !list.iter().any(|existing| existing.name().ok() == def.name().ok()) {
                                list.push(def);
                            }
                        }
                        if let Ok(devs) = host.output_devices() {
                            for d in devs {
                                let name = d.name().unwrap_or_default();
                                if name.contains("Steam") || name.contains("Virtual") || name.contains("Null") {
                                    continue;
                                }
                                if !list.iter().any(|existing| existing.name().ok() == d.name().ok()) {
                                    list.push(d);
                                }
                            }
                        }
                        list
                    };

                    for dev in devices_to_try {
                        let dev_name = dev.name().unwrap_or_else(|_| "Dispositivo Desconhecido".to_string());
                        let config_res = dev.default_output_config();
                        let config = match config_res {
                            Ok(c) => c,
                            Err(e) => {
                                warn!("⚠️ [LOOPBACK TX] Falha ao obter configuração para '{}': {:?}", dev_name, e);
                                continue;
                            }
                        };
                        let sample_rate = config.sample_rate().0;
                        let channels = config.channels() as usize;

                        let pcm_buf = Arc::clone(&pcm_buffer_cb);
                        let dev_name_err = dev_name.clone();
                        let err_fn = move |err| {
                            warn!("Erro na captura de áudio loopback ({}): {}", dev_name_err, err);
                        };

                        let stream_res = match config.sample_format() {
                            cpal::SampleFormat::F32 => {
                                dev.build_input_stream(
                                    &config.into(),
                                    move |data: &[f32], _| {
                                        let mut buf = pcm_buf.lock().unwrap_or_else(|e| e.into_inner());
                                        if channels == 1 {
                                            for &s in data {
                                                let sample_i16 = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                                                buf.push(sample_i16);
                                            }
                                        } else if channels >= 2 {
                                            for chunk in data.chunks_exact(channels) {
                                                let mono_f = (chunk[0] + chunk[1]) * 0.5;
                                                let sample_i16 = (mono_f.clamp(-1.0, 1.0) * 32767.0) as i16;
                                                buf.push(sample_i16);
                                            }
                                        }
                                    },
                                    err_fn,
                                    None,
                                )
                            }
                            cpal::SampleFormat::I16 => {
                                dev.build_input_stream(
                                    &config.into(),
                                    move |data: &[i16], _| {
                                        let mut buf = pcm_buf.lock().unwrap_or_else(|e| e.into_inner());
                                        if channels == 1 {
                                            buf.extend_from_slice(data);
                                        } else if channels >= 2 {
                                            for chunk in data.chunks_exact(channels) {
                                                let mono = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                                                buf.push(mono);
                                            }
                                        }
                                    },
                                    err_fn,
                                    None,
                                )
                            }
                            _ => {
                                warn!("Formato de áudio não suportado para '{}'", dev_name);
                                continue;
                            }
                        };

                        if let Ok(stream) = stream_res {
                            if stream.play().is_ok() {
                                active_sample_rate = sample_rate;
                                target_chunk_samples = ((sample_rate as usize) * 10) / 1000;
                                info!("🔊 [LOOPBACK TX] Captura de áudio ATIVA no dispositivo '{}' ({}Hz, {} canais, chunk={} amostras/10ms)!", dev_name, sample_rate, channels, target_chunk_samples);
                                active_stream = Some(stream);
                                break;
                            }
                        }
                    }
                }
            }

            #[cfg(target_os = "windows")]
            if active_stream.is_none() && wasapi_isolated_handle.is_none() {
                warn!("⚠️ [LOOPBACK TX] Falha ao ativar stream de áudio em todos os dispositivos disponíveis!");
                return;
            }

            #[cfg(target_os = "linux")]
            if gst_child.is_none() {
                warn!("⚠️ [LOOPBACK TX] Falha ao ativar captura de áudio do sistema via GStreamer pulsesrc!");
                return;
            }

            let socket = get_shared_p2p_socket().unwrap_or_else(|| {
                let s = UdpSocket::bind("0.0.0.0:0").unwrap();
                let _ = s.set_broadcast(true);
                let _ = s.set_nonblocking(true);
                Arc::new(s)
            });

            let mut pkts_sent_count: u64 = 0;
            while is_running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(8));

                let chunks_to_send: Vec<Vec<i16>> = {
                    let mut buf = pcm_buffer.lock().unwrap_or_else(|e| e.into_inner());
                    let mut res = Vec::new();
                    // Prevent TX buffer accumulation
                    if buf.len() > target_chunk_samples * 6 {
                        let excess = buf.len() - target_chunk_samples * 2;
                        buf.drain(0..excess);
                        let fade_len = 32.min(buf.len());
                        for i in 0..fade_len {
                            let factor = i as f32 / fade_len as f32;
                            buf[i] = (buf[i] as f32 * factor) as i16;
                        }
                    }
                    while buf.len() >= target_chunk_samples && target_chunk_samples > 0 {
                        let chunk: Vec<i16> = buf.drain(0..target_chunk_samples).collect();
                        res.push(chunk);
                    }
                    res
                };

                let mut cid = channel_id.load(Ordering::Relaxed);
                if cid == 0 {
                    cid = crate::gateway::get_my_voice_channel_id();
                }
                let mut uid = my_user_id.load(Ordering::Relaxed);
                if uid == 0 {
                    uid = crate::gateway::get_my_user_id();
                }
                let inst = get_process_instance_id();

                for chunk in chunks_to_send {
                    seq = seq.wrapping_add(1);
                    let pts_ms = get_tx_pts_ms();
                    let sample_count = chunk.len() as u16;
                    let mut raw_pcm = Vec::with_capacity(chunk.len() * 2);
                    for &s in &chunk {
                        raw_pcm.extend_from_slice(&s.to_le_bytes());
                    }
                    let sec_key = get_voice_encryption_key(cid);
                    let audio_payload = encrypt_signaling_payload(&sec_key, &raw_pcm).unwrap_or(raw_pcm);
                    let mut pkt = Vec::with_capacity(36 + audio_payload.len());
                    pkt.extend_from_slice(MAGIC);
                    pkt.extend_from_slice(&inst.to_be_bytes());
                    pkt.push(OP_AUDIO_FRAME);
                    pkt.extend_from_slice(&cid.to_be_bytes());
                    pkt.extend_from_slice(&uid.to_be_bytes());
                    pkt.extend_from_slice(&seq.to_be_bytes());
                    pkt.extend_from_slice(&pts_ms.to_be_bytes());
                    pkt.push(1); // 1 channel
                    pkt.extend_from_slice(&active_sample_rate.to_be_bytes());
                    pkt.extend_from_slice(&sample_count.to_be_bytes());
                    pkt.extend_from_slice(&audio_payload);

                    let mut target_addrs: Vec<SocketAddr> = Vec::with_capacity(8);
                    if let Ok(peers) = peers_store.lock() {
                        for (&_p_key, &(addr, _)) in peers.iter() {
                            if !is_tailscale_or_forbidden(&addr) && !target_addrs.contains(&addr) {
                                target_addrs.push(addr);
                            }
                        }
                    }
                    if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                        if let Some(map) = last_guard.as_ref() {
                            for (&p_uid, &addr) in map.iter() {
                                if p_uid != uid && !is_tailscale_or_forbidden(&addr) && !target_addrs.contains(&addr) {
                                    target_addrs.push(addr);
                                }
                            }
                        }
                    }
                    for target in &target_addrs {
                        let _ = socket.send_to(&pkt, target);
                    }

                    let mut peak_tx: i16 = 0;
                    for &s in &chunk {
                        let abs = s.saturating_abs();
                        if abs > peak_tx { peak_tx = abs; }
                    }

                    pkts_sent_count += 1;
                    if pkts_sent_count % 50 == 1 {
                        info!("📡 [LOOPBACK TX] Pkt #{} enviado ({} amostras @ {}Hz | Peak Amplitude: {}/32767) para {:?}", pkts_sent_count, target_chunk_samples, active_sample_rate, peak_tx, target_addrs);
                    }
                }
            }

            #[cfg(target_os = "windows")]
            {
                drop(active_stream);
                drop(wasapi_isolated_handle);
            }

            #[cfg(target_os = "linux")]
            if let Some(mut child) = gst_child {
                let _ = child.kill();
                let _ = child.wait();
            }
            info!("🔊 [LOOPBACK TX] Captura de áudio da transmissão finalizada.");
        })
        .expect("Falha ao iniciar thread de captura de áudio da transmissão");
}
