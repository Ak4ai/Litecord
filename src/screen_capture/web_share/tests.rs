use super::*;

#[test]
fn signaling_authenticates_key_and_rejects_tampering() {
    let key = [7; 32];
    let payload = signal::encrypt(&key, b"private SDP").unwrap();
    assert_eq!(signal::decrypt(&key, &payload).unwrap(), b"private SDP");
    assert!(signal::decrypt(&[8; 32], &payload).is_none());
    let mut bad = payload.into_bytes(); bad[20] = if bad[20] == b'A' { b'B' } else { b'A' };
    assert!(signal::decrypt(&key, std::str::from_utf8(&bad).unwrap()).is_none());
    assert!(signal::decrypt(&key, "AA==").is_none());
}

#[test]
fn opus_accepts_loopback_rates_with_continuous_timestamps() {
    for rate in [44100, 48000] {
        let mut encoder = audio::AudioEncoder::new().unwrap();
        let chunk = vec![1000; rate / 100];
        let packets: Vec<_> = (0..100).flat_map(|_| encoder.push(&chunk, rate as u32)).collect();
        assert_eq!(packets.len(), 50);
        let mut decoder = opus_rs::OpusDecoder::new(48000, 1).unwrap();
        for (i, (packet, timestamp)) in packets.iter().enumerate() {
            assert_eq!(*timestamp, i as u64 * 960);
            let mut pcm = vec![0.0f32; 960];
            assert_eq!(decoder.decode(packet, 960, &mut pcm).unwrap(), 960);
        }
    }
}

#[test]
fn idr_detection_accepts_three_and_four_byte_start_codes() {
    assert!(contains_idr(&[0, 0, 0, 1, 0x65, 1]));
    assert!(contains_idr(&[0, 0, 1, 0x65, 1]));
    assert!(!contains_idr(&[0, 0, 1, 0x41, 1]));
}

/// Controlled test source: no login, microphone, real screen or external messages.
/// Run via scripts/perf/web_share_e2e.cjs with a local Wrangler worker.
#[test]
#[ignore]
fn browser_fixture() {
    let _ = env_logger::builder().is_test(true).try_init();
    let url = start().expect("start web sharing");
    let output = std::env::var("LITECORD_WEB_TEST_READY").expect("test runner ready path");
    std::fs::write(&output, serde_json::json!({"url": url}).to_string()).unwrap();
    let mut encoder = openh264::encoder::Encoder::new().unwrap();
    let mut rgb = vec![0u8; 640 * 360 * 3];
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < Duration::from_secs(180) && !std::path::Path::new(&format!("{output}.stop")).exists() {
        if let Ok(path) = std::env::var("LITECORD_WEB_TEST_EXCHANGE") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) {
                    let _ = std::fs::remove_file(&path);
                    let answer = serde_json::from_value(request["offer"].clone())
                        .map_err(|e| e.to_string()).and_then(answer_manual);
                    let response = match answer {
                        Ok(answer) => serde_json::json!({"id":request["id"],"answer":answer}),
                        Err(error) => serde_json::json!({"id":request["id"],"error":error}),
                    };
                    std::fs::write(format!("{path}.answer"), response.to_string()).unwrap();
                }
            }
        }
        let tick = Instant::now();
        if n % 4 == 0 && viewer_count() > 0 {
            for (i, pixel) in rgb.chunks_exact_mut(3).enumerate() {
                pixel.copy_from_slice(&[(i % 640) as u8, (i / 640) as u8, n as u8]);
            }
            let yuv = openh264::formats::YUVBuffer::from_rgb_source(openh264::formats::RgbSliceU8::new(&rgb, (640, 360)));
            // Use all-intra frames in this fixture; production uses the shared encoder's IDRs.
            encoder.force_intra_frame();
            let frame = encoder.encode(&yuv).unwrap().to_vec();
            offer_frame(&frame);
        }
        let audio: Vec<i16> = (0..960).map(|i| {
            let t = (n * 960 + i) as f32 / 48000.0;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 6000.0) as i16
        }).collect();
        offer_audio(&audio, 48000);
        n += 1;
        std::thread::sleep(Duration::from_millis(20).saturating_sub(tick.elapsed()));
    }
    stop();
    assert!(!is_active());
    assert_eq!(viewer_count(), 0);
    assert!(FRAMES.lock().unwrap().is_none());
}
