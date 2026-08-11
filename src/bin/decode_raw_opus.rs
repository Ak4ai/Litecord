use opus_rs::OpusDecoder;
use std::fs::File;
use std::io::{Read, Write};

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

fn main() {
    println!("==================================================");
    println!("🔬 DECODIFICADOR ISOLADO DE PACOTES BRUTOS OPUS");
    println!("==================================================");
    let path = "discord_raw_opus.bin";
    let mut f = match File::open(path) {
        Ok(file) => file,
        Err(e) => {
            println!("❌ Não foi possível abrir `discord_raw_opus.bin`: {}. Execute o app Litecord primeiro para gravar pacotes brutos!", e);
            return;
        }
    };

    let mut raw_bytes = Vec::new();
    let _ = f.read_to_end(&mut raw_bytes);
    println!("Lidos {} bytes de dados brutos Opus de `discord_raw_opus.bin`.", raw_bytes.len());

    let mut decoder = OpusDecoder::new(48000, 2).expect("Falha ao criar OpusDecoder 48kHz");
    let mut pcm_out_buf = vec![0.0f32; 11520];
    let mut decoded_pcm = Vec::new();

    let mut idx = 0;
    let mut pkt_count = 0;
    let mut err_count = 0;

    while idx + 2 <= raw_bytes.len() {
        let pkt_len = u16::from_le_bytes([raw_bytes[idx], raw_bytes[idx + 1]]) as usize;
        idx += 2;
        if idx + pkt_len > raw_bytes.len() {
            break;
        }
        let pkt = &raw_bytes[idx..idx + pkt_len];
        idx += pkt_len;
        pkt_count += 1;

        if let Ok(samples) = decoder.decode(pkt, 5760, &mut pcm_out_buf[..]) {
            for i in 0..samples {
                let l = pcm_out_buf[i * 2].clamp(-1.0, 1.0);
                let r = pcm_out_buf[i * 2 + 1].clamp(-1.0, 1.0);
                decoded_pcm.push((l, r));
            }
        } else {
            err_count += 1;
        }
    }

    println!("Decodificados {} quadros Opus ({} erros). Total de amostras: {}", pkt_count, err_count, decoded_pcm.len());

    let out_path = "discord_raw_decoded.wav";
    let mut out_f = File::create(out_path).expect("Falha ao criar WAV de saída");
    let data_bytes = decoded_pcm.len() * 4;
    write_wav_header(&mut out_f, 48000, 2, data_bytes as u32);
    let mut pcm_bytes = Vec::with_capacity(data_bytes);
    for &(l, r) in &decoded_pcm {
        let i_l = (l * 32767.0) as i16;
        let i_r = (r * 32767.0) as i16;
        pcm_bytes.extend_from_slice(&i_l.to_le_bytes());
        pcm_bytes.extend_from_slice(&i_r.to_le_bytes());
    }
    let _ = out_f.write_all(&pcm_bytes);
    println!("✅ Áudio decodificado e salvo em `{}`!", out_path);
    println!("Execute `python scratch_spectral_analysis.py discord_raw_decoded.wav` para inspecionar!");
}
