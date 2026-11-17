use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use log::{info, warn};

/// Directory for custom soundboard files (.wav, .pcm)
pub fn get_soundboard_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let path = PathBuf::from(app_data).join("Litecord").join("sounds");
            let _ = std::fs::create_dir_all(&path);
            return path;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(home).join("Library").join("Application Support").join("Litecord").join("sounds");
            let _ = std::fs::create_dir_all(&path);
            return path;
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            let path = PathBuf::from(xdg).join("litecord").join("sounds");
            let _ = std::fs::create_dir_all(&path);
            return path;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home).join(".config").join("litecord").join("sounds");
        let _ = std::fs::create_dir_all(&path);
        return path;
    }
    PathBuf::from("sounds")
}

/// Synthesize rich 48kHz procedural sound effects for instant zero-latency playback
pub fn generate_soundboard_pcm(sound_id: &str) -> Vec<f32> {
    let sample_rate = 48000.0f32;
    let dt = 1.0 / sample_rate;

    match sound_id.to_lowercase().as_str() {
        "airhorn" => {
            // Iconic 3-stage multi-tone fanfare
            let mut out = Vec::new();
            let blasts = [(0.14, 466.16), (0.14, 466.16), (0.45, 622.25)]; // Bb4, Bb4, Eb5
            for (dur, freq) in blasts {
                let count = (sample_rate * dur) as usize;
                let mut phase = 0.0f32;
                for i in 0..count {
                    let t = i as f32 * dt;
                    phase += 2.0 * std::f32::consts::PI * freq * dt;
                    // Brass harmonic richness
                    let sample = 0.5 * phase.sin()
                        + 0.25 * (phase * 2.0).sin()
                        + 0.15 * (phase * 3.0).sin()
                        + 0.10 * (phase * 4.0).sin();
                    let env = if t < 0.015 { t / 0.015 } else { (-(t - 0.015) * 2.0).exp().clamp(0.0, 1.0) };
                    out.push(sample * env * 0.75);
                }
                // Short silence gap between blasts
                out.extend(vec![0.0f32; (sample_rate * 0.05) as usize]);
            }
            out
        }
        "bruh" => {
            // Resonant low vocal drop: formant sweep 140Hz -> 85Hz
            let duration = 0.65;
            let count = (sample_rate * duration) as usize;
            let mut out = Vec::with_capacity(count);
            let mut phase = 0.0f32;
            for i in 0..count {
                let t = i as f32 / count as f32;
                let pitch = 140.0 - 55.0 * t.powf(1.4);
                phase += 2.0 * std::f32::consts::PI * pitch * dt;
                let formant1 = (phase * 3.2).sin() * 0.35;
                let formant2 = (phase * 6.5).sin() * 0.18;
                let sample = (phase.sin() + formant1 + formant2) * 0.65;
                let env = (-(t - 0.08).max(0.0) * 4.0).exp();
                out.push((sample * env).clamp(-1.0, 1.0));
            }
            out
        }
        "quack" => {
            // Squeaky duck formant sweep
            let duration = 0.40;
            let count = (sample_rate * duration) as usize;
            let mut out = Vec::with_capacity(count);
            let mut phase = 0.0f32;
            for i in 0..count {
                let t = i as f32 / count as f32;
                let pitch = 380.0 + 80.0 * (1.0 - t).powf(2.0);
                phase += 2.0 * std::f32::consts::PI * pitch * dt;
                let mod_wave = (2.0 * std::f32::consts::PI * 45.0 * (i as f32 * dt)).sin();
                let sample = (phase + mod_wave * 0.6).sin() * 0.70;
                let env = (-(t - 0.04).max(0.0) * 6.0).exp();
                out.push((sample * env).clamp(-1.0, 1.0));
            }
            out
        }
        "badumtss" => {
            // Ba (kick/tom) -> Dum (snare) -> Tss (crash cymbal)
            let mut out = Vec::new();
            // Ba (0.15s)
            let count1 = (sample_rate * 0.15) as usize;
            let mut phase = 0.0f32;
            for i in 0..count1 {
                let t = i as f32 / count1 as f32;
                let pitch = 180.0 - 100.0 * t;
                phase += 2.0 * std::f32::consts::PI * pitch * dt;
                let env = (-t * 6.0).exp();
                out.push(phase.sin() * env * 0.7);
            }
            // Gap (0.08s)
            out.extend(vec![0.0f32; (sample_rate * 0.08) as usize]);
            // Dum (0.18s)
            let count2 = (sample_rate * 0.18) as usize;
            for i in 0..count2 {
                let t = i as f32 / count2 as f32;
                let pitch = 140.0 - 80.0 * t;
                phase += 2.0 * std::f32::consts::PI * pitch * dt;
                let noise = ((i * 1103515245 + 12345) & 0x7FFFFFFF) as f32 / 0x7FFFFFFF as f32 - 0.5;
                let env = (-t * 5.0).exp();
                out.push((phase.sin() * 0.5 + noise * 0.4) * env * 0.75);
            }
            // Gap (0.06s)
            out.extend(vec![0.0f32; (sample_rate * 0.06) as usize]);
            // Tss (0.55s crash)
            let count3 = (sample_rate * 0.55) as usize;
            for i in 0..count3 {
                let t = i as f32 / count3 as f32;
                let noise = ((i * 1664525 + 1013904223) & 0x7FFFFFFF) as f32 / 0x7FFFFFFF as f32 - 0.5;
                let env = (-t * 4.5).exp();
                out.push(noise * env * 0.65);
            }
            out
        }
        "vineboom" => {
            // Saturated 808 sub-bass explosion
            let duration = 1.1;
            let count = (sample_rate * duration) as usize;
            let mut out = Vec::with_capacity(count);
            let mut phase = 0.0f32;
            for i in 0..count {
                let t = i as f32 / count as f32;
                let pitch = 135.0 * (-t * 3.2).exp() + 38.0;
                phase += 2.0 * std::f32::consts::PI * pitch * dt;
                // Add soft saturation distortion for the classic 808 boom punch
                let raw = (phase.sin() * 1.8 + (phase * 0.5).sin() * 0.4).tanh();
                let noise = (((i * 214013 + 2531011) & 0x7FFFFFFF) as f32 / 0x7FFFFFFF as f32 - 0.5) * (-t * 12.0).exp();
                let env = (-t * 2.8).exp();
                out.push((raw * 0.8 + noise * 0.35) * env);
            }
            out
        }
        "tada" => {
            // Bright triumphant fanfare arpeggio (C5 -> E5 -> G5 -> C6)
            let notes = [523.25, 659.25, 783.99, 1046.50];
            let mut out = Vec::new();
            for (idx, &freq) in notes.iter().enumerate() {
                let dur = if idx == 3 { 0.65 } else { 0.12 };
                let count = (sample_rate * dur) as usize;
                let mut phase = 0.0f32;
                for i in 0..count {
                    let t = i as f32 / count as f32;
                    phase += 2.0 * std::f32::consts::PI * freq * dt;
                    let sample = phase.sin() * 0.6 + (phase * 2.0).sin() * 0.25;
                    let env = if idx == 3 { (-t * 2.8).exp() } else { (-t * 4.0).exp() };
                    out.push(sample * env * 0.7);
                }
            }
            out
        }
        "applause" => {
            // Clapping audience noise bursts
            let duration = 1.4;
            let count = (sample_rate * duration) as usize;
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let t = i as f32 / count as f32;
                let noise = ((i * 1103515245 + 12345) & 0x7FFFFFFF) as f32 / 0x7FFFFFFF as f32 - 0.5;
                let mod_rate = (2.0 * std::f32::consts::PI * 7.5 * (i as f32 * dt)).sin().abs();
                let env = (-t * 1.5).exp();
                out.push(noise * mod_rate * env * 0.65);
            }
            out
        }
        "cricket" => {
            // Awkward cricket chirps
            let mut out = Vec::new();
            for _ in 0..3 {
                let count = (sample_rate * 0.08) as usize;
                let mut phase = 0.0f32;
                for i in 0..count {
                    let t = i as f32 / count as f32;
                    phase += 2.0 * std::f32::consts::PI * 4500.0 * dt;
                    let amp = (phase.sin()).abs() * (2.0 * std::f32::consts::PI * 60.0 * (i as f32 * dt)).sin();
                    let env = (-t * 5.0).exp();
                    out.push(amp * env * 0.5);
                }
                out.extend(vec![0.0f32; (sample_rate * 0.15) as usize]);
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Plays soundboard audio into the voice channel (via mic queue injection)
/// and simultaneously plays it back locally so the user hears it!
pub fn play_soundboard_effect(sound_id: &str) {
    let sound_name = sound_id.to_string();
    info!("📢 [SOUNDBOARD] Disparando efeito sonoro '{}' sem restrições de Nitro/duração!", sound_name);

    std::thread::Builder::new()
        .name(format!("soundboard-{}", sound_name))
        .spawn(move || {
            // 1. First check if a custom user WAV exists in soundboard dir
            let custom_path = get_soundboard_dir().join(format!("{}.wav", sound_name));
            let samples = if custom_path.exists() {
                load_wav_48k(&custom_path).unwrap_or_else(|_| generate_soundboard_pcm(&sound_name))
            } else {
                generate_soundboard_pcm(&sound_name)
            };

            if samples.is_empty() {
                warn!("⚠️ [SOUNDBOARD] Nenhum sample gerado para {}", sound_name);
                return;
            }

            // 2. Stream into voice mic queue in standard Discord 20ms frames (960 samples @ 48kHz)
            let chunk_size = 960;
            let pcm_queue = crate::gateway::get_mic_pcm_queue();

            for chunk in samples.chunks(chunk_size) {
                if let Ok(mut q) = pcm_queue.lock() {
                    for &s in chunk {
                        if q.len() < 96000 {
                            q.push_back(s.clamp(-1.0, 1.0));
                        }
                    }
                }
                crate::gateway::MIC_FRAME_READY.notify_one();

                // Also feed into local loopback so the local user hears their soundboard
                if let Ok(mut loop_q) = crate::gateway::get_mic_loopback_queue().lock() {
                    for &s in chunk {
                        if loop_q.len() < 96000 {
                            loop_q.push_back(s.clamp(-1.0, 1.0));
                        }
                    }
                }

                // Throttle pacing to 20ms per frame
                std::thread::sleep(Duration::from_millis(19));
            }

            info!("✅ [SOUNDBOARD] Efeito sonoro '{}' transmitido com sucesso!", sound_name);
        })
        .expect("Falha ao iniciar thread de soundboard");
}

/// Simple 16-bit PCM WAV loader fallback with auto-resampling to 48kHz
fn load_wav_48k(path: &std::path::Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Formato WAV inválido".into());
    }

    let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as usize;
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as f32;
    let bits_per_sample = u16::from_le_bytes([bytes[34], bytes[35]]);

    if bits_per_sample != 16 || channels == 0 {
        return Err("Apenas WAV 16-bit PCM suportado".into());
    }

    let data_offset = 44;
    let data_bytes = &bytes[data_offset..];
    let num_samples = data_bytes.len() / 2;
    let mut mono_samples = Vec::with_capacity(num_samples / channels);

    for chunk in data_bytes.chunks_exact(2 * channels) {
        let mut sum = 0.0f32;
        for c in 0..channels {
            let val = i16::from_le_bytes([chunk[c * 2], chunk[c * 2 + 1]]);
            sum += val as f32 / 32768.0;
        }
        mono_samples.push(sum / channels as f32);
    }

    // Resample if not 48kHz
    if (sample_rate - 48000.0).abs() > 1.0 {
        let ratio = 48000.0 / sample_rate;
        let out_len = (mono_samples.len() as f32 * ratio) as usize;
        let mut resampled = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src_idx = (i as f32 / ratio) as usize;
            if src_idx < mono_samples.len() {
                resampled.push(mono_samples[src_idx]);
            }
        }
        Ok(resampled)
    } else {
        Ok(mono_samples)
    }
}
