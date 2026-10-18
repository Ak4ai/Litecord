use super::*;

#[test]
fn stop_does_not_wait_for_close_and_retired_session_cannot_change_restart() {
    use std::net::{TcpListener, TcpStream};
    use tokio_tungstenite::tungstenite::{accept, client, Message};

    // The worker accepts the WebSocket, but withholds its close acknowledgement.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (release_server, server_release) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = accept(stream).unwrap();
        ws.send(Message::Text("ready".into())).unwrap();
        let _ = server_release.recv_timeout(Duration::from_secs(5));
    });
    let old_state = Arc::new(SessionState::default());
    old_state.active.store(true, Ordering::Release);
    old_state.viewers.store(2, Ordering::Relaxed);
    let old_worker_state = Arc::clone(&old_state);
    let old_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let old_port = old_socket.local_addr().unwrap().port();
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (closing_tx, closing_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let old_thread = std::thread::spawn(move || {
        let _socket_owner = old_socket;
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (mut ws, _) = client(format!("ws://{addr}"), tcp).unwrap();
        ws.read().unwrap();
        ready_tx.send(()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async { let _ = stop_rx.changed().await; });
        let _ = ws.close(None);
        closing_tx.send(()).unwrap();
        let _ = ws.read(); // Deliberately stalled network cleanup.
        *old_worker_state.error.lock().unwrap() = "retired connection closed".into();
        old_worker_state.active.store(false, Ordering::Release);
        old_worker_state.viewers.store(0, Ordering::Relaxed);
        status_changed();
        let _ = done_tx.send(());
    });
    ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    *SESSION.lock().unwrap() = Some(WebShare {
        url: "old-session".into(), state: Arc::clone(&old_state), stop: stop_tx, thread: old_thread,
    });
    HAS_SESSION.store(true, Ordering::Release);
    let started = Instant::now();
    stop();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(100), "stop blocked for {elapsed:?}");
    closing_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(done_rx.try_recv().is_err(), "the mock worker must still be waiting for close");
    assert!(!is_active());

    let new_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    assert_ne!(old_port, new_socket.local_addr().unwrap().port());
    let new_state = Arc::new(SessionState::default());
    new_state.active.store(true, Ordering::Release);
    new_state.viewers.store(3, Ordering::Relaxed);
    let (new_stop, mut new_stop_rx) = watch::channel(false);
    let (new_done_tx, new_done_rx) = std::sync::mpsc::channel();
    let new_thread = std::thread::spawn(move || {
        let _socket_owner = new_socket;
        tokio::runtime::Builder::new_current_thread().build().unwrap()
            .block_on(async { let _ = new_stop_rx.changed().await; });
        let _ = new_done_tx.send(());
    });
    let (new_frames, _frames_rx) = mpsc::channel(2);
    *SESSION.lock().unwrap() = Some(WebShare {
        url: "new-session".into(), state: Arc::clone(&new_state), stop: new_stop, thread: new_thread,
    });
    *FRAMES.lock().unwrap() = Some(new_frames.clone());
    HAS_SESSION.store(true, Ordering::Release);
    // A failed old start must not cancel the new session either.
    stop_if_current(Some(&old_state));
    release_server.send(()).unwrap();
    server.join().unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(is_active());
    assert_eq!(viewer_count(), 3);
    assert_eq!(last_error(), "");
    assert!(FRAMES.lock().unwrap().as_ref().unwrap().same_channel(&new_frames));
    assert!(Arc::ptr_eq(&SESSION.lock().unwrap().as_ref().unwrap().state, &new_state));
    stop();
    new_done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    println!("stop without close acknowledgement: {:.3} ms; restarted session remained active with 3 viewers", elapsed.as_secs_f64() * 1000.0);
}

#[test]
fn viewer_verifier_matches_browser_hash_and_is_bound_to_the_room() {
    assert_eq!(signal::viewer_verifier(&[7; 32], &"a".repeat(32)),
        "caf39d2291ee9e4bb85dcba118c6982f27910239756f848898cd28b30db626a6");
    assert_ne!(signal::viewer_verifier(&[8; 32], &"a".repeat(32)),
        signal::viewer_verifier(&[7; 32], &"a".repeat(32)));
    assert_ne!(signal::viewer_verifier(&[7; 32], &"b".repeat(32)),
        signal::viewer_verifier(&[7; 32], &"a".repeat(32)));
}

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
