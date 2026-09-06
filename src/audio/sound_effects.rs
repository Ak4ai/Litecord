use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use log::{info, warn};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiSound {
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    JoinChannel,
    LeaveChannel,
}

static SOUND_SENDER: std::sync::OnceLock<SyncSender<UiSound>> = std::sync::OnceLock::new();

/// Initializes the background audio thread for instant, zero-latency UI sound playback.
pub fn init_sound_effects() {
    if SOUND_SENDER.get().is_some() {
        return;
    }

    let (tx, rx) = sync_channel::<UiSound>(16);
    let _ = SOUND_SENDER.set(tx);

    std::thread::Builder::new()
        .name("ui-sound-player".to_string())
        .spawn(move || {
            info!("🔊 [SOUND EFFECTS] Motor de áudio tátil iniciado com sucesso via CPAL");

            while let Ok(sound) = rx.recv() {
                // Drain any excessive queued sounds to only play the latest if flooded
                let mut current_sound = sound;
                while let Ok(next) = rx.try_recv() {
                    current_sound = next;
                }

                if let Err(e) = play_sound_cpal(current_sound) {
                    warn!("⚠️ [SOUND EFFECTS] Falha na reprodução CPAL ({:?}), tentando fallback nativo...", e);
                    play_sound_fallback(current_sound);
                }
            }
        })
        .expect("Falha ao iniciar thread de áudio de efeitos sonoros");
}

/// Plays a tactile UI sound effect immediately.
pub fn play_ui_sound(sound: UiSound) {
    if let Some(sender) = SOUND_SENDER.get() {
        let _ = sender.try_send(sound);
    } else {
        init_sound_effects();
        if let Some(sender) = SOUND_SENDER.get() {
            let _ = sender.try_send(sound);
        } else {
            play_sound_fallback(sound);
        }
    }
}

fn generate_sound_samples(sound: UiSound, sample_rate: u32) -> Vec<f32> {
    let dt = 1.0 / sample_rate as f32;
    let mut samples = Vec::new();

    match sound {
        UiSound::Mute => {
            // Descending tactile blip: 540Hz -> 280Hz (~120ms)
            let duration = 0.12;
            let total = (sample_rate as f32 * duration) as usize;
            let mut phase = 0.0f32;
            for i in 0..total {
                let t = i as f32 / total as f32;
                let freq = 540.0 - 260.0 * (t * t);
                phase += 2.0 * std::f32::consts::PI * freq * dt;

                let attack = (i as f32 / (sample_rate as f32 * 0.006)).min(1.0);
                let decay = (-t * 3.8).exp();
                let wave = (phase.sin() + 0.20 * (phase * 2.0).sin()) * 0.70 * attack * decay;
                samples.push(wave);
            }
        }
        UiSound::Unmute => {
            // Ascending bright chirp: 320Hz -> 640Hz (~120ms)
            let duration = 0.12;
            let total = (sample_rate as f32 * duration) as usize;
            let mut phase = 0.0f32;
            for i in 0..total {
                let t = i as f32 / total as f32;
                let freq = 320.0 + 320.0 * t.sqrt();
                phase += 2.0 * std::f32::consts::PI * freq * dt;

                let attack = (i as f32 / (sample_rate as f32 * 0.006)).min(1.0);
                let decay = (-t * 3.5).exp();
                let wave = (phase.sin() + 0.22 * (phase * 2.0).sin()) * 0.75 * attack * decay;
                samples.push(wave);
            }
        }
        UiSound::Deafen => {
            // Two-step downward tone: Note 1 (480Hz->380Hz, 85ms), Note 2 (340Hz->200Hz, 100ms)
            let duration = 0.19;
            let total = (sample_rate as f32 * duration) as usize;
            let split = (sample_rate as f32 * 0.085) as usize;
            let mut phase1 = 0.0f32;
            let mut phase2 = 0.0f32;

            for i in 0..total {
                let wave = if i < split {
                    let t = i as f32 / split as f32;
                    let freq = 480.0 - 100.0 * t;
                    phase1 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (i as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 2.8).exp();
                    (phase1.sin() + 0.18 * (phase1 * 2.0).sin()) * 0.70 * attack * decay
                } else {
                    let j = i - split;
                    let rem = total - split;
                    let t = j as f32 / rem as f32;
                    let freq = 340.0 - 140.0 * (t * t);
                    phase2 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (j as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 3.2).exp();
                    (phase2.sin() + 0.18 * (phase2 * 2.0).sin()) * 0.75 * attack * decay
                };
                samples.push(wave);
            }
        }
        UiSound::Undeafen => {
            // Two-step upward chime: Note 1 (260Hz->380Hz, 85ms), Note 2 (420Hz->700Hz, 100ms)
            let duration = 0.19;
            let total = (sample_rate as f32 * duration) as usize;
            let split = (sample_rate as f32 * 0.085) as usize;
            let mut phase1 = 0.0f32;
            let mut phase2 = 0.0f32;

            for i in 0..total {
                let wave = if i < split {
                    let t = i as f32 / split as f32;
                    let freq = 260.0 + 120.0 * t;
                    phase1 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (i as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 2.8).exp();
                    (phase1.sin() + 0.18 * (phase1 * 2.0).sin()) * 0.70 * attack * decay
                } else {
                    let j = i - split;
                    let rem = total - split;
                    let t = j as f32 / rem as f32;
                    let freq = 420.0 + 280.0 * t.sqrt();
                    phase2 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (j as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 3.0).exp();
                    (phase2.sin() + 0.22 * (phase2 * 2.0).sin()) * 0.78 * attack * decay
                };
                samples.push(wave);
            }
        }
        UiSound::JoinChannel => {
            // Ascending warm chime: Note 1 (440Hz, 85ms), Note 2 (660Hz, 130ms)
            let duration = 0.215;
            let total = (sample_rate as f32 * duration) as usize;
            let split = (sample_rate as f32 * 0.085) as usize;
            let mut phase1 = 0.0f32;
            let mut phase2 = 0.0f32;

            for i in 0..total {
                let wave = if i < split {
                    let t = i as f32 / split as f32;
                    let freq = 440.0;
                    phase1 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (i as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 2.5).exp();
                    (phase1.sin() + 0.15 * (phase1 * 2.0).sin()) * 0.75 * attack * decay
                } else {
                    let j = i - split;
                    let rem = total - split;
                    let t = j as f32 / rem as f32;
                    let freq = 660.0;
                    phase2 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (j as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 3.0).exp();
                    (phase2.sin() + 0.18 * (phase2 * 2.0).sin()) * 0.80 * attack * decay
                };
                samples.push(wave);
            }
        }
        UiSound::LeaveChannel => {
            // Descending warm chime: Note 1 (660Hz, 85ms), Note 2 (440Hz, 130ms)
            let duration = 0.215;
            let total = (sample_rate as f32 * duration) as usize;
            let split = (sample_rate as f32 * 0.085) as usize;
            let mut phase1 = 0.0f32;
            let mut phase2 = 0.0f32;

            for i in 0..total {
                let wave = if i < split {
                    let t = i as f32 / split as f32;
                    let freq = 660.0;
                    phase1 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (i as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 2.8).exp();
                    (phase1.sin() + 0.15 * (phase1 * 2.0).sin()) * 0.75 * attack * decay
                } else {
                    let j = i - split;
                    let rem = total - split;
                    let t = j as f32 / rem as f32;
                    let freq = 440.0;
                    phase2 += 2.0 * std::f32::consts::PI * freq * dt;
                    let attack = (j as f32 / (sample_rate as f32 * 0.005)).min(1.0);
                    let decay = (-t * 3.2).exp();
                    (phase2.sin() + 0.15 * (phase2 * 2.0).sin()) * 0.75 * attack * decay
                };
                samples.push(wave);
            }
        }
    }

    samples
}

fn play_sound_cpal(sound: UiSound) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = cpal::default_host();
    let target_dev_name = if let Ok(guard) = crate::gateway::get_selected_output_device_store().lock() {
        guard.clone()
    } else {
        String::new()
    };
    let device = if !target_dev_name.is_empty() && !target_dev_name.contains("Padrão") {
        host.output_devices().ok().and_then(|mut devs| devs.find(|d| d.name().map_or(false, |n| n == target_dev_name)))
            .or_else(|| host.default_output_device())
    } else {
        host.default_output_device()
    }.ok_or_else(|| "Nenhum dispositivo de áudio de saída encontrado")?;

    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    let samples = generate_sound_samples(sound, sample_rate);
    let total_samples = samples.len();
    let samples_arc = Arc::new(samples);

    let current_idx = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let playback_finished = Arc::new(AtomicBool::new(false));

    let idx_cb = Arc::clone(&current_idx);
    let finish_cb = Arc::clone(&playback_finished);
    let smp_cb = Arc::clone(&samples_arc);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let idx = idx_cb.fetch_add(1, Ordering::Relaxed);
                    let val = if idx < total_samples {
                        smp_cb[idx]
                    } else {
                        finish_cb.store(true, Ordering::Relaxed);
                        0.0
                    };
                    for sample in frame.iter_mut() {
                        *sample = val;
                    }
                }
            },
            |err| warn!("Erro no stream de som CPAL: {}", err),
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.into(),
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let idx = idx_cb.fetch_add(1, Ordering::Relaxed);
                    let val = if idx < total_samples {
                        smp_cb[idx]
                    } else {
                        finish_cb.store(true, Ordering::Relaxed);
                        0.0
                    };
                    let val_i16 = (val * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    for sample in frame.iter_mut() {
                        *sample = val_i16;
                    }
                }
            },
            |err| warn!("Erro no stream de som CPAL: {}", err),
            None,
        )?,
        _ => return Err("Formato de áudio não suportado para UI sounds".into()),
    };

    stream.play()?;

    // Wait until playback completes with 250ms timeout safety
    let max_wait = ((total_samples as f32 / sample_rate as f32) * 1000.0) as u64 + 40;
    let start = std::time::Instant::now();
    while !playback_finished.load(Ordering::Relaxed) && start.elapsed() < Duration::from_millis(max_wait) {
        std::thread::sleep(Duration::from_millis(5));
    }

    std::thread::sleep(Duration::from_millis(10));
    drop(stream);
    Ok(())
}

fn play_sound_fallback(sound: UiSound) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Media::Audio::{PlaySoundA, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

        let sample_rate = 44100u32;
        let samples = generate_sound_samples(sound, sample_rate);
        let mut wav = Vec::with_capacity(44 + samples.len() * 2);

        let data_size = (samples.len() * 2) as u32;
        let file_size = 36 + data_size;

        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());

        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());

        for s in samples {
            let s_i16 = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
            wav.extend_from_slice(&s_i16.to_le_bytes());
        }

        unsafe {
            PlaySoundA(
                wav.as_ptr(),
                0 as _,
                SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
            );
        }
    }

    #[cfg(not(windows))]
    {
        let sample_rate = 44100u32;
        let samples = generate_sound_samples(sound, sample_rate);
        let mut wav = Vec::with_capacity(44 + samples.len() * 2);

        let data_size = (samples.len() * 2) as u32;
        let file_size = 36 + data_size;

        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());

        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());

        for s in samples {
            let s_i16 = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
            wav.extend_from_slice(&s_i16.to_le_bytes());
        }

        for player in &["pw-play", "paplay", "aplay"] {
            if let Ok(mut child) = std::process::Command::new(player)
                .arg("-")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(&wav);
                }
                let _ = child.wait();
                break;
            }
        }
    }
}
