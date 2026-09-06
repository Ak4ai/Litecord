#![allow(dead_code)]

use tokio::sync::mpsc;

use std::sync::Arc;
use std::collections::VecDeque;
use cpal::traits::{HostTrait, DeviceTrait, StreamTrait};
use log::info;
use crate::gateway;

pub static CACHED_AUDIO_DEVICES: std::sync::OnceLock<Arc<std::sync::Mutex<Option<(Vec<String>, Vec<String>)>>>> = std::sync::OnceLock::new();

pub fn get_audio_devices_cache() -> Arc<std::sync::Mutex<Option<(Vec<String>, Vec<String>)>>> {
    CACHED_AUDIO_DEVICES.get_or_init(|| Arc::new(std::sync::Mutex::new(None))).clone()
}

pub fn enumerate_audio_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    if let Ok(devices) = host.input_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                if !inputs.contains(&name) {
                    inputs.push(name);
                }
            }
        }
    }

    if let Ok(devices) = host.output_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                if !outputs.contains(&name) {
                    outputs.push(name);
                }
            }
        }
    }

    if inputs.is_empty() {
        inputs.push("Microfone Padrão do Sistema".to_string());
    }

    if outputs.is_empty() {
        outputs.push("Alto-falantes Padrão do Sistema".to_string());
    }

    let result = (inputs, outputs);
    if let Ok(mut cache) = get_audio_devices_cache().lock() {
        *cache = Some(result.clone());
    }
    result
}

pub fn push_to_mic_queues(pcm_q: &mut VecDeque<f32>, pcm_samples: &[f32], loop_samples: &[f32], level: f32) {
    for &s in pcm_samples {
        let clamped = s.clamp(-1.0, 1.0);
        if pcm_q.len() < 96000 { pcm_q.push_back(clamped); }
    }
    if gateway::is_testing_mic() {
        let vad = gateway::get_vad_threshold();
        let is_active = level >= vad;
        pub static LAST_LOG: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
        let log_mtx = LAST_LOG.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
        if let Ok(mut last) = log_mtx.lock() {
            if last.elapsed().as_millis() >= 1000 {
                *last = std::time::Instant::now();
                info!("🧪 [TEST MIC STATS] mic_level={:.3}, vad_threshold={:.3}, active={}", level, vad, is_active);
            }
        }
        if let Ok(mut loop_q) = gateway::get_mic_loopback_queue().lock() {
            if loop_q.len() > 6000 {
                let excess = loop_q.len() - 3500;
                loop_q.drain(0..excess);
            }
            for &s in loop_samples {
                let sample_to_play = if is_active { s.clamp(-1.0, 1.0) } else { 0.0 };
                loop_q.push_back(sample_to_play);
            }
        }
    }
}

pub fn start_mic_capture(
    device_name: String,
    level_tx: mpsc::Sender<f32>,
) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let devices = host.input_devices().ok()?;

    let target_dev = if device_name.is_empty() || device_name.contains("Padrão") {
        host.default_input_device()?
    } else {
        devices.into_iter().find(|d| {
            d.name().map(|n| n == device_name).unwrap_or(false)
        }).or_else(|| host.default_input_device())?
    };

    let config = target_dev.default_input_config().ok()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let pcm_queue = gateway::get_mic_pcm_queue();

    info!("Capturando microfone: {}Hz, {} canal(is), formato={:?}", sample_rate, channels, config.sample_format());

    let stream_config: cpal::StreamConfig = config.clone().into();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &_| {
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            push_to_mic_queues(&mut q, &mono_samples, &mono_samples, level);
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            let mut resampled_buf = Vec::with_capacity(out_len);
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = s0 * (1.0 - frac) + s1 * frac;
                                resampled_buf.push(resampled);
                            }
                            push_to_mic_queues(&mut q, &resampled_buf, &mono_samples, level);
                        }
                    }
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone: {:?}", err);
                },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I16 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &stream_config,
                move |data: &[i16], _: &_| {
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            push_to_mic_queues(&mut q, &mono_samples, &mono_samples, level);
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            let mut resampled_buf = Vec::with_capacity(out_len);
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = s0 * (1.0 - frac) + s1 * frac;
                                resampled_buf.push(resampled);
                            }
                            push_to_mic_queues(&mut q, &resampled_buf, &mono_samples, level);
                        }
                    }
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone I16: {:?}", err);
                },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I32 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &stream_config,
                move |data: &[i32], _: &_| {
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().map(|&x| x as f32 / 2147483648.0).sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            push_to_mic_queues(&mut q, &mono_samples, &mono_samples, level);
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            let mut resampled_buf = Vec::with_capacity(out_len);
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = s0 * (1.0 - frac) + s1 * frac;
                                resampled_buf.push(resampled);
                            }
                            push_to_mic_queues(&mut q, &resampled_buf, &mono_samples, level);
                        }
                    }
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone I32: {:?}", err);
                },
                None,
            ).ok()?
        }
        _ => return None,
    };

    if stream.play().is_ok() {
        info!("🎙️ Stream de Microfone INICIADO COM SUCESSO! Rate: {}Hz -> 48000Hz", sample_rate);
        Some(stream)
    } else {
        log::error!("Falha ao executar stream.play() no microfone!");
        None
    }
}

pub fn start_mic_loopback_stream(device_name: String) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let devices = host.output_devices().ok()?;

    let target_dev = if device_name.is_empty() || device_name.contains("Padrão") {
        host.default_output_device()?
    } else {
        devices.into_iter().find(|d| {
            d.name().map(|n| n == device_name).unwrap_or(false)
        }).or_else(|| host.default_output_device())?
    };

    let target_dev_name = target_dev.name().unwrap_or_else(|_| "desconhecido".to_string());
    let config = match target_dev.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            log::error!("❌ [LOOPBACK] Erro ao obter default_output_config para '{}': {:?}", target_dev_name, e);
            return None;
        }
    };
    let out_sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    let loop_q_arc = gateway::get_mic_loopback_queue();

    info!("🎧 [LOOPBACK] Iniciando no dispositivo '{}': {}Hz, {} canal(is), formato={:?}", target_dev_name, out_sample_rate, channels, config.sample_format());

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let q_arc = Arc::clone(&loop_q_arc);
            let mut is_started = false;
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [f32], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 0.0; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        if !is_started {
                            if loop_q.len() < 2000 {
                                for s in output.iter_mut() { *s = 0.0; }
                                return;
                            }
                            is_started = true;
                        }
                        let frames = output.len() / channels.max(1);
                        let mut sum_sq = 0.0f32;
                        for f in 0..frames {
                            let s = loop_q.pop_front().unwrap_or(0.0);
                            let sample = (s * 3.0).clamp(-1.0, 1.0);
                            sum_sq += sample * sample;
                            for ch in 0..channels {
                                output[f * channels + ch] = sample;
                            }
                        }
                        let out_rms = (sum_sq / frames.max(1) as f32).sqrt();
                        pub static LAST_OUT_LOG: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
                        let out_log_mtx = LAST_OUT_LOG.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
                        if let Ok(mut last) = out_log_mtx.lock() {
                            if last.elapsed().as_millis() >= 1000 {
                                *last = std::time::Instant::now();
                                info!("🎧 [LOOPBACK PLAYBACK F32] is_started={}, q_len={}, frames={}, out_rms={:.4}", is_started, loop_q.len(), frames, out_rms);
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 0.0; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback F32: {:?}", err); },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I16 => {
            let q_arc = Arc::clone(&loop_q_arc);
            let mut is_started = false;
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [i16], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 0; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        if !is_started {
                            if loop_q.len() < 2000 {
                                for s in output.iter_mut() { *s = 0; }
                                return;
                            }
                            is_started = true;
                        }
                        let frames = output.len() / channels.max(1);
                        let mut sum_sq = 0.0f32;
                        for f in 0..frames {
                            let s = loop_q.pop_front().unwrap_or(0.0);
                            let sample = (s * 3.0).clamp(-1.0, 1.0);
                            sum_sq += sample * sample;
                            let val = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                            for ch in 0..channels {
                                output[f * channels + ch] = val;
                            }
                        }
                        let out_rms = (sum_sq / frames.max(1) as f32).sqrt();
                        pub static LAST_OUT_LOG: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
                        let out_log_mtx = LAST_OUT_LOG.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
                        if let Ok(mut last) = out_log_mtx.lock() {
                            if last.elapsed().as_millis() >= 1000 {
                                *last = std::time::Instant::now();
                                info!("🎧 [LOOPBACK PLAYBACK I16] is_started={}, q_len={}, frames={}, out_rms={:.4}", is_started, loop_q.len(), frames, out_rms);
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 0; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback I16: {:?}", err); },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I32 => {
            let q_arc = Arc::clone(&loop_q_arc);
            let mut is_started = false;
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [i32], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 0; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        if !is_started {
                            if loop_q.len() < 2000 {
                                for s in output.iter_mut() { *s = 0; }
                                return;
                            }
                            is_started = true;
                        }
                        let frames = output.len() / channels.max(1);
                        let mut sum_sq = 0.0f32;
                        for f in 0..frames {
                            let s = loop_q.pop_front().unwrap_or(0.0);
                            let sample = (s * 3.0).clamp(-1.0, 1.0);
                            sum_sq += sample * sample;
                            let val = (sample * 2147483647.0).clamp(-2147483648.0, 2147483647.0) as i32;
                            for ch in 0..channels {
                                output[f * channels + ch] = val;
                            }
                        }
                        let out_rms = (sum_sq / frames.max(1) as f32).sqrt();
                        pub static LAST_OUT_LOG: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
                        let out_log_mtx = LAST_OUT_LOG.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
                        if let Ok(mut last) = out_log_mtx.lock() {
                            if last.elapsed().as_millis() >= 1000 {
                                *last = std::time::Instant::now();
                                info!("🎧 [LOOPBACK PLAYBACK I32] is_started={}, q_len={}, frames={}, out_rms={:.4}", is_started, loop_q.len(), frames, out_rms);
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 0; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback I32: {:?}", err); },
                None,
            ).ok()?
        }
        cpal::SampleFormat::U16 => {
            let q_arc = Arc::clone(&loop_q_arc);
            let mut is_started = false;
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [u16], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 32768; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        if !is_started {
                            if loop_q.len() < 2000 {
                                for s in output.iter_mut() { *s = 32768; }
                                return;
                            }
                            is_started = true;
                        }
                        let frames = output.len() / channels.max(1);
                        let mut sum_sq = 0.0f32;
                        for f in 0..frames {
                            let s = loop_q.pop_front().unwrap_or(0.0);
                            let sample = (s * 3.0).clamp(-1.0, 1.0);
                            sum_sq += sample * sample;
                            let val = ((sample + 1.0) * 32767.5).clamp(0.0, 65535.0) as u16;
                            for ch in 0..channels {
                                output[f * channels + ch] = val;
                            }
                        }
                        let out_rms = (sum_sq / frames.max(1) as f32).sqrt();
                        pub static LAST_OUT_LOG: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
                        let out_log_mtx = LAST_OUT_LOG.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
                        if let Ok(mut last) = out_log_mtx.lock() {
                            if last.elapsed().as_millis() >= 1000 {
                                *last = std::time::Instant::now();
                                info!("🎧 [LOOPBACK PLAYBACK U16] is_started={}, q_len={}, frames={}, out_rms={:.4}", is_started, loop_q.len(), frames, out_rms);
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 32768; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback U16: {:?}", err); },
                None,
            ).ok()?
        }
        _ => return None,
    };

    if stream.play().is_ok() {
        info!("🎧 Stream Loopback ('Se Ouvir') iniciado com sucesso!");
        Some(stream)
    } else {
        None
    }
}

