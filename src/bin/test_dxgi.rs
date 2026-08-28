use openh264::encoder::{Encoder, EncoderConfig, UsageType, RateControlMode};
use openh264::formats::YUVSlices;
use std::time::Instant;

fn main() {
    let w = 1920usize;
    let h = 1080usize;
    let u_base = w * h;
    let v_base = u_base / 4;
    let yuv_buffer = vec![128u8; u_base + v_base * 2];

    for threads in [2, 4, 6, 8, 12, 16] {
        let enc_config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .set_multiple_thread_idc(threads)
            .set_bitrate_bps(5_000_000)
            .max_frame_rate(60.0)
            .enable_skip_frame(false)
            .rate_control_mode(RateControlMode::Quality);

        if let Ok(mut encoder) = Encoder::with_api_config(openh264::OpenH264API::from_source(), enc_config) {
            let (y_plane, uv_plane) = yuv_buffer.split_at(u_base);
            let (u_plane, v_plane) = uv_plane.split_at(v_base);
            let yuv_slices = YUVSlices::new((y_plane, u_plane, v_plane), (w, h), (w, w / 2, w / 2));

            // Warm up
            let _ = encoder.encode(&yuv_slices);

            let t0 = Instant::now();
            for _ in 0..10 {
                let _ = encoder.encode(&yuv_slices);
            }
            let dur = t0.elapsed() / 10;
            println!("Threads={:2} -> Encode time: {:.2} ms ({:.1} FPS max)", threads, dur.as_secs_f64() * 1000.0, 1.0 / dur.as_secs_f64());
        }
    }
}
