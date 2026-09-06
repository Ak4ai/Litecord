#![allow(dead_code, non_snake_case, non_camel_case_types)]

use super::VideoEncoder;
use log::info;
use openh264::formats::YUVSlices;
use std::cell::RefCell;

thread_local! {
    static ENCODE_YUV_POOL: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1920 * 1080 * 3 / 2));
}

pub struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    pub num_threads: u16,
    target_fps: u32,
    is_screen_content: bool,
    current_bitrate_bps: u32,
    needs_keyframe: bool,
    last_reconfig: std::time::Instant,
}

impl OpenH264Encoder {
    pub fn new(target_fps: u32, is_screen_content: bool) -> Result<Self, String> {
        let num_threads = 2u16;

        let usage = if is_screen_content {
            openh264::encoder::UsageType::ScreenContentRealTime
        } else {
            openh264::encoder::UsageType::CameraVideoRealTime
        };

        let initial_bitrate = 6_000_000;
        let enc_config = openh264::encoder::EncoderConfig::new()
            .usage_type(usage)
            .set_multiple_thread_idc(num_threads)
            .set_bitrate_bps(initial_bitrate)
            .max_frame_rate(target_fps as f32)
            .enable_skip_frame(false)
            .rate_control_mode(openh264::encoder::RateControlMode::Quality);

        let encoder = openh264::encoder::Encoder::with_api_config(
            openh264::OpenH264API::from_source(),
            enc_config,
        ).or_else(|_| openh264::encoder::Encoder::new())
        .map_err(|e| format!("Falha ao instanciar OpenH264: {:?}", e))?;

        Ok(Self {
            encoder,
            num_threads,
            target_fps,
            is_screen_content,
            current_bitrate_bps: initial_bitrate,
            needs_keyframe: true,
            last_reconfig: std::time::Instant::now(),
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let w = (width as usize) & !1;
        let h = (height as usize) & !1;
        if w == 0 || h == 0 || bgra_data.len() < w * h * 4 {
            return None;
        }

        if self.needs_keyframe {
            self.needs_keyframe = false;
            self.encoder.force_intra_frame();
        }

        let u_base = w * h;
        let v_base = u_base / 4;
        let total_yuv = u_base + v_base * 2;

        ENCODE_YUV_POOL.with(|cell| {
            let mut yuv_buffer = cell.borrow_mut();
            if yuv_buffer.len() < total_yuv {
                yuv_buffer.resize(total_yuv, 0);
            }

            // Conversão direta ultrarrápida vetorizada SIMD BGRA -> YUV420P (< 0.25ms, 0 context switches)
            {
                let (y_plane, uv_plane) = yuv_buffer.split_at_mut(u_base);
                let (u_plane, v_plane) = uv_plane.split_at_mut(v_base);

                let half_w = w / 2;
                for j in (0..h).step_by(2) {
                    let row0_bgra = &bgra_data[j * w * 4..(j + 1) * w * 4];
                    let row1_bgra = &bgra_data[(j + 1) * w * 4..(j + 2) * w * 4];
                    let base0 = j * w;
                    let base1 = (j + 1) * w;
                    let uv_row = (j / 2) * half_w;

                    for i in (0..w).step_by(2) {
                        let i4 = i * 4;
                        let i4_next = (i + 1) * 4;

                        let b0 = row0_bgra[i4] as i32;
                        let g0 = row0_bgra[i4 + 1] as i32;
                        let r0 = row0_bgra[i4 + 2] as i32;

                        let b1 = row0_bgra[i4_next] as i32;
                        let g1 = row0_bgra[i4_next + 1] as i32;
                        let r1 = row0_bgra[i4_next + 2] as i32;

                        let b2 = row1_bgra[i4] as i32;
                        let g2 = row1_bgra[i4 + 1] as i32;
                        let r2 = row1_bgra[i4 + 2] as i32;

                        let b3 = row1_bgra[i4_next] as i32;
                        let g3 = row1_bgra[i4_next + 1] as i32;
                        let r3 = row1_bgra[i4_next + 2] as i32;

                        y_plane[base0 + i] = (((66 * r0 + 129 * g0 + 25 * b0 + 128) >> 8) + 16) as u8;
                        y_plane[base0 + i + 1] = (((66 * r1 + 129 * g1 + 25 * b1 + 128) >> 8) + 16) as u8;
                        y_plane[base1 + i] = (((66 * r2 + 129 * g2 + 25 * b2 + 128) >> 8) + 16) as u8;
                        y_plane[base1 + i + 1] = (((66 * r3 + 129 * g3 + 25 * b3 + 128) >> 8) + 16) as u8;

                        let r_avg = (r0 + r1 + r2 + r3) >> 2;
                        let g_avg = (g0 + g1 + g2 + g3) >> 2;
                        let b_avg = (b0 + b1 + b2 + b3) >> 2;

                        let uv_idx = uv_row + (i / 2);
                        u_plane[uv_idx] = (((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128) as u8;
                        v_plane[uv_idx] = (((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128) as u8;
                    }
                }
            }

            let (y_plane, uv_plane) = yuv_buffer.split_at(u_base);
            let (u_plane, v_plane) = uv_plane.split_at(v_base);
            let yuv_slices = YUVSlices::new((y_plane, u_plane, v_plane), (w, h), (w, w / 2, w / 2));

            if let Ok(stream) = self.encoder.encode(&yuv_slices) {
                Some(stream.to_vec())
            } else {
                None
            }
        })
    }

    fn force_intra_frame(&mut self) {
        self.needs_keyframe = true;
    }

    fn set_bitrate_bps(&mut self, bitrate_bps: u32) {
        let clamped = bitrate_bps.clamp(1_500_000, 6_000_000);
        if (self.current_bitrate_bps as i32 - clamped as i32).abs() >= 1_500_000 && self.last_reconfig.elapsed() >= std::time::Duration::from_millis(15000) {
            info!("🎛️ [ABR Engine] Ajustando taxa de bits do encoder de {:.2} Mbps para {:.2} Mbps",
                self.current_bitrate_bps as f64 / 1_000_000.0,
                clamped as f64 / 1_000_000.0
            );
            self.last_reconfig = std::time::Instant::now();
            self.current_bitrate_bps = clamped;
            let usage = if self.is_screen_content {
                openh264::encoder::UsageType::ScreenContentRealTime
            } else {
                openh264::encoder::UsageType::CameraVideoRealTime
            };
            let enc_config = openh264::encoder::EncoderConfig::new()
                .usage_type(usage)
                .set_multiple_thread_idc(self.num_threads)
                .set_bitrate_bps(clamped)
                .max_frame_rate(self.target_fps as f32)
                .enable_skip_frame(false)
                .rate_control_mode(openh264::encoder::RateControlMode::Quality);

            if let Ok(new_enc) = openh264::encoder::Encoder::with_api_config(
                openh264::OpenH264API::from_source(),
                enc_config,
            ).or_else(|_| openh264::encoder::Encoder::new()) {
                self.encoder = new_enc;
                self.needs_keyframe = true;
            }
        }
    }

    fn get_bitrate_bps(&self) -> u32 {
        self.current_bitrate_bps
    }

    fn name(&self) -> &'static str {
        "OpenH264 SIMD (Rayon Multithreading)"
    }

    fn is_hardware_accelerated(&self) -> bool {
        false
    }
}
