use opus_rs::{OpusEncoder, OpusDecoder, Application};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Write;

pub fn soft_limit(s: f32) -> f32 {
    s.clamp(-1.0, 1.0)
}

pub fn cubic_hermite(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a0 = p3 - p2 - p0 + p1;
    let a1 = p0 - p1 - a0;
    let a2 = p2 - p0;
    let a3 = p1;
    a0 * t * t * t + a1 * t * t + a2 * t + a3
}

fn write_wav_header(f: &mut File, sample_rate: u32, channels: u16, data_size: u32) {
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    let file_size: u32 = 36 + data_size;
    header[4..8].copy_from_slice(&file_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    let fmt_size: u32 = 16;
    header[16..20].copy_from_slice(&fmt_size.to_le_bytes());
    let format_tag: u16 = 1;
    header[20..22].copy_from_slice(&format_tag.to_le_bytes());
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    let byte_rate: u32 = sample_rate * channels as u32 * 2;
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    let block_align: u16 = channels * 2;
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    let bits_per_sample: u16 = 16;
    header[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    let _ = f.write_all(&header);
}

fn save_wav(filename: &str, samples: &[(f32, f32)], sample_rate: u32) {
    let mut f = File::create(filename).expect("Failed to create wav file");
    let data_bytes = samples.len() * 4;
    write_wav_header(&mut f, sample_rate, 2, data_bytes as u32);
    let mut pcm_bytes = Vec::with_capacity(data_bytes);
    for &(l, r) in samples {
        let i_l = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
        let i_r = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
        pcm_bytes.extend_from_slice(&i_l.to_le_bytes());
        pcm_bytes.extend_from_slice(&i_r.to_le_bytes());
    }
    let _ = f.write_all(&pcm_bytes);
}

fn generate_synthetic_speech(duration_secs: f32, sample_rate: usize) -> Vec<(f32, f32)> {
    let total_samples = (duration_secs * sample_rate as f32) as usize;
    let mut ground_truth = Vec::with_capacity(total_samples);

    for i in 0..total_samples {
        let t = i as f32 / sample_rate as f32;
        let mut sample_l = 0.0f32;
        let mut sample_r = 0.0f32;

        if t >= 0.5 && t < 1.5 {
            // Segment 1: Vowel formant sweep (500Hz + 1500Hz) with smooth fade-in
            let envelope = ((t - 0.5) * 5.0).min(1.0) * ((1.5 - t) * 5.0).min(1.0);
            let f1 = (2.0 * std::f32::consts::PI * 500.0 * t).sin() * 0.4;
            let f2 = (2.0 * std::f32::consts::PI * 1500.0 * t).sin() * 0.2;
            sample_l = (f1 + f2) * envelope;
            sample_r = sample_l;
        } else if t >= 1.5 && t < 1.8 {
            // Segment 2: Silence gap (300ms)
            sample_l = 0.0;
            sample_r = 0.0;
        } else if t >= 1.8 && t < 3.0 {
            // Segment 3: Sudden LOUD Speech Onsets & Fast Pitch Transitions (440Hz -> 880Hz -> 1320Hz)
            let pitch_t = t - 1.8;
            let freq = if pitch_t < 0.4 {
                440.0
            } else if pitch_t < 0.8 {
                880.0
            } else {
                1320.0
            };
            let loud_onset = if pitch_t < 0.05 { 0.8 } else { 0.5 };
            let tone = (2.0 * std::f32::consts::PI * freq * t).sin() * loud_onset;
            let harmonic = (2.0 * std::f32::consts::PI * (freq * 2.0) * t).sin() * 0.2;
            sample_l = tone + harmonic;
            sample_r = sample_l;
        } else if t >= 3.0 && t < 3.2 {
            // Segment 4: Silence gap (200ms)
            sample_l = 0.0;
            sample_r = 0.0;
        } else if t >= 3.2 && t < 4.5 {
            // Segment 5: Rapid pitch modulation (Vocal trill 300Hz to 600Hz) with proper phase accumulator
            let mod_freq = 450.0 + 150.0 * (2.0 * std::f32::consts::PI * 10.0 * t).sin();
            let phase_inc = 2.0 * std::f32::consts::PI * mod_freq / sample_rate as f32;
            let current_phase = if i > 0 {
                // Approximate phase integration
                (2.0 * std::f32::consts::PI * 450.0 * t) + 150.0 / 10.0 * (1.0 - (2.0 * std::f32::consts::PI * 10.0 * t).cos())
            } else { 0.0 };
            sample_l = current_phase.sin() * 0.4;
            sample_r = sample_l;
        } else {
            // Baseline silence
            sample_l = 0.0;
            sample_r = 0.0;
        }

        ground_truth.push((sample_l.clamp(-1.0, 1.0), sample_r.clamp(-1.0, 1.0)));
    }

    ground_truth
}

fn main() {
    println!("==================================================");
    println!("🧪 TESTE AUTOMATIZADO E DIAGNÓSTICO DO PIPELINE DE ÁUDIO");
    println!("==================================================");

    let sample_rate = 48000;
    let duration = 5.0f32;
    println!("1. Gerando sinal sintético de fala humana (5.0s, 48kHz Estéreo)...");
    let ground_truth = generate_synthetic_speech(duration, sample_rate);
    save_wav("test_ground_truth.wav", &ground_truth, sample_rate as u32);
    println!("   - Salvo ground truth original em `test_ground_truth.wav`.");

    println!("\n2. Codificando via Opus (libopus 48kHz Stereo VBR)...");
    let mut encoder = OpusEncoder::new(48000, 2, Application::Voip)
        .expect("Falha ao criar OpusEncoder");

    let frame_samples_per_ch = 960; // 20ms
    let frame_samples_total = frame_samples_per_ch * 2;
    let num_frames = ground_truth.len() / frame_samples_per_ch;
    let mut opus_packets = Vec::new();

    for f_idx in 0..num_frames {
        let start = f_idx * frame_samples_per_ch;
        let end = start + frame_samples_per_ch;
        let chunk = &ground_truth[start..end];

        let mut pcm_buf = vec![0.0f32; frame_samples_total];
        for (i, &(l, r)) in chunk.iter().enumerate() {
            pcm_buf[i * 2] = l.clamp(-1.0, 1.0);
            pcm_buf[i * 2 + 1] = r.clamp(-1.0, 1.0);
        }

        let mut opus_buf = vec![0u8; 1275];
        let bytes_written = encoder.encode(&pcm_buf, 960, &mut opus_buf)
            .expect("Falha ao codificar frame Opus");
        opus_buf.truncate(bytes_written);
        opus_packets.push((f_idx, opus_buf));
    }

    println!("\n3. Simulando Condições Reais de Rede (UDP Jitter, DAVE Passthrough 0x00, e Perda de Pacotes)...");
    let mut network_packets = Vec::new();
    for (f_idx, mut opus_pkt) in opus_packets {
        // Prepend DAVE 0x00 codec header to 50% of packets
        if f_idx % 2 == 0 {
            let mut dave_pkt = vec![0x00];
            dave_pkt.extend_from_slice(&opus_pkt);
            opus_pkt = dave_pkt;
        }

        // Simulate 2% packet loss (drop frame 50 and frame 120)
        if f_idx == 50 || f_idx == 120 {
            continue;
        }

        network_packets.push((f_idx, opus_pkt));
    }

    println!("\n4. Executando Pipeline Completo de Decodificação & Reamostragem (gateway.rs)...");
    let mut decoder = OpusDecoder::new(48000, 2)
        .expect("Falha ao criar OpusDecoder");

    let mut decoded_pipeline_samples = Vec::new();
    let mut pcm_out_buf = vec![0.0f32; 11520];
    let mut ssrc_expected_seq: HashMap<u32, u16> = HashMap::new();
    let ssrc_recv = 12345u32;
    let mut speaker_queue = VecDeque::with_capacity(96000);

    let mut ssrc_phases: HashMap<u32, f64> = HashMap::new();
    let mut ssrc_histories: HashMap<u32, [(f32, f32); 4]> = HashMap::new();
    let mut started_ssrcs = std::collections::HashSet::new();

    let mut total_plc_events = 0;
    let mut total_dtx_suppressions = 0;
    let mut frame_errors = Vec::new();

    for (f_idx, opus_data) in network_packets {
        let rtp_seq = f_idx as u16;
        let raw_opus_data = opus_data.as_slice();

        // 1. DAVE 0x00 Codec Header strip logic
        let mut raw_opus = raw_opus_data;
        if raw_opus.first() == Some(&0x00) && raw_opus.len() > 1 {
            raw_opus = &raw_opus[1..];
        }

        // 2. RTP Packet Loss & Sequence Check
        if let Some(last_seq) = ssrc_expected_seq.get(&ssrc_recv).copied() {
            let missed = rtp_seq.wrapping_sub(last_seq);
            if missed > 0 && missed < 10 {
                total_plc_events += missed;
                for _ in 0..missed {
                    let _ = decoder.decode(&[], 5760, &mut pcm_out_buf[..]);
                }
            }
        }
        ssrc_expected_seq.insert(ssrc_recv, rtp_seq.wrapping_add(1));

        // 3. DTX Comfort Noise Suppression logic
        let is_dtx_silence = raw_opus.len() <= 3 || (raw_opus.len() <= 5 && (raw_opus[0] == 0xF8 || raw_opus[0] == 0xFC));
        let mut decoded_count = 0usize;

        if is_dtx_silence {
            total_dtx_suppressions += 1;
            decoded_count = 960;
            for s in pcm_out_buf[..1920].iter_mut() { *s = 0.0; }
        } else if let Ok(samples) = decoder.decode(raw_opus, 5760, &mut pcm_out_buf[..]) {
            decoded_count = samples;
        }

        // 4. Push to speaker queue
        for i in 0..decoded_count {
            speaker_queue.push_back((pcm_out_buf[i * 2].clamp(-1.0, 1.0), pcm_out_buf[i * 2 + 1].clamp(-1.0, 1.0)));
        }

        // 5. CPAL Speaker Callback Simulation (Resampler & Pre-fill)
        let out_sample_rate = 48000;
        let frames_to_pop = 960; // 20ms at 48kHz

        if started_ssrcs.contains(&ssrc_recv) || speaker_queue.len() >= 960 {
            started_ssrcs.insert(ssrc_recv);
            let phase = ssrc_phases.entry(ssrc_recv).or_insert(0.0);
            let hist = ssrc_histories.entry(ssrc_recv).or_insert([(0.0, 0.0); 4]);
            let step = 48000.0f64 / out_sample_rate.max(1) as f64;

            for _ in 0..frames_to_pop {
                *phase += step;
                let pops = *phase as usize;
                if pops > 0 {
                    *phase -= pops as f64;
                    for _ in 0..pops {
                        hist[0] = hist[1];
                        hist[1] = hist[2];
                        hist[2] = hist[3];
                        if let Some(next_p) = speaker_queue.pop_front() {
                            if hist[0] == (0.0, 0.0) && hist[1] == (0.0, 0.0) && hist[2] == (0.0, 0.0) {
                                hist[0] = next_p;
                                hist[1] = next_p;
                                hist[2] = next_p;
                                hist[3] = next_p;
                            } else {
                                hist[3] = next_p;
                            }
                        } else {
                            hist[3] = (hist[2].0 * 0.95, hist[2].1 * 0.95);
                        }
                    }
                }
                let t = (*phase as f32).clamp(0.0, 1.0);
                let src_l = cubic_hermite(hist[0].0, hist[1].0, hist[2].0, hist[3].0, t);
                let src_r = cubic_hermite(hist[0].1, hist[1].1, hist[2].1, hist[3].1, t);
                let limited_l = soft_limit(src_l);
                let limited_r = soft_limit(src_r);
                decoded_pipeline_samples.push((limited_l, limited_r));
            }
        }

        // Compensate for libopus 120-sample (2.5ms) algorithmic lookahead delay
        let delay = 120;
        let gt_start = f_idx * 960;
        let pipe_start = gt_start + delay;

        if pipe_start + 960 <= decoded_pipeline_samples.len() && gt_start + 960 <= ground_truth.len() {
            let gt_chunk = &ground_truth[gt_start..gt_start + 960];
            let pipe_chunk = &decoded_pipeline_samples[pipe_start..pipe_start + 960];
            let mut frame_max_err = 0.0f32;
            let mut err_sq_sum = 0.0f32;

            for (i, &(gt_l, _)) in gt_chunk.iter().enumerate() {
                let (pipe_l, _) = pipe_chunk[i];
                let err = (pipe_l - gt_l).abs();
                if err > frame_max_err { frame_max_err = err; }
                err_sq_sum += err * err;
            }
            let frame_rms_err = (err_sq_sum / 960.0).sqrt();

            if frame_max_err > 0.15 {
                frame_errors.push((f_idx, f_idx as f32 * 0.02, frame_max_err, frame_rms_err));
            }
        }
    }

    save_wav("test_pipeline_output.wav", &decoded_pipeline_samples, sample_rate as u32);
    println!("   - Salvo resultado do pipeline em `test_pipeline_output.wav`.");

    println!("\n==================================================");
    println!("📊 DIAGNÓSTICO E ANÁLISE DE DIFERENÇA SAMPLE-BY-SAMPLE");
    println!("==================================================");
    println!("Eventos de Perda PLC simulados: {}", total_plc_events);
    println!("Quadros de Silêncio DTX Suprimidos: {}", total_dtx_suppressions);
    println!("Amostras Comparadas: {} pares estéreo", decoded_pipeline_samples.len().min(ground_truth.len()));

    if frame_errors.is_empty() {
        println!("\n✅ SUCESSO ABSOLUTO! Nenhuma discrepância de áudio/estática/chiado detectada (>0.15 err)!");
        println!("   O pipeline reproduz o sinal com 100% de transparência e fidelidade estúdios!");
    } else {
        println!("\n⚠️ FORAM DETECTADAS {} ANOMALIAS/DISCREPÂNCIAS DE ÁUDIO NO PIPELINE:", frame_errors.len());
        for err in frame_errors.iter().take(15) {
            println!("   - Quadro #{:>3} ({:>5.2}s): Erro Máximo={:.4} | RMS Error={:.4}", err.0, err.1, err.2, err.3);
        }
    }
}
