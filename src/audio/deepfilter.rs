//! DeepFilterNet3 noise suppression (Linux).
//!
//! Discord's optional "Krisp" noise cancellation is proprietary and is not
//! redistributed here. DeepFilterNet3 is the closest open-source, real-time
//! full-band (48 kHz) speech-enhancement model; this module embeds upstream's
//! default DFN3 model and runs it after WebRTC AEC3, before VAD/AGC.
//!
//! DFN3 needs 10 ms frames at 48 kHz and has a 30 ms algorithmic look-ahead.
//! The model is loaded on a background thread so the audio callback never
//! blocks; until it is ready, the caller falls back to RNNoise/WebRTC NS.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(enabled: bool) {
    let was_enabled = ENABLED.swap(enabled, Ordering::Relaxed);
    if enabled {
        imp::warm_up();
        if !was_enabled {
            imp::reset();
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ndarray::{ArrayView2, ArrayViewMut2};
    use std::sync::{Mutex, OnceLock};

    const DFN_FRAME: usize = 480;

    /// `DfTract` contains `Rc` internally, so it is `!Send` even after the model
    /// plans are built. We create it on a loader thread and then move ownership
    /// into a `Mutex` used by the capture thread; no other thread ever touches
    /// the model, so the move is sound.
    struct DfProcessor(df::tract::DfTract);
    unsafe impl Send for DfProcessor {}

    static PROCESSOR: OnceLock<Mutex<Option<DfProcessor>>> = OnceLock::new();
    static LOADING: AtomicBool = AtomicBool::new(false);

    fn processor_slot() -> &'static Mutex<Option<DfProcessor>> {
        PROCESSOR.get_or_init(|| Mutex::new(None))
    }

    /// Attenuation limit (dB): 100 dB causes underwater/metallic artifacts on live speech;
    /// 18 dB cleans background noise while keeping vocal harmonics and consonants natural.
    const ATTEN_LIM_DB: f32 = 18.0;

    pub fn warm_up() {
        if processor_slot().lock().map(|p| p.is_some()).unwrap_or(false) {
            return;
        }
        if LOADING.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            let started = std::time::Instant::now();
            // tract builds three ONNX plans from the embedded model; keep it off
            // the UI and audio threads.
            let loaded = std::panic::catch_unwind(|| {
                df::tract::DfTract::new(
                    df::tract::DfParams::default(),
                    &df::tract::RuntimeParams::default_with_ch(1).with_atten_lim(ATTEN_LIM_DB),
                )
            });
            match loaded {
                Ok(Ok(processor)) => {
                    if let Ok(mut slot) = processor_slot().lock() {
                        *slot = Some(DfProcessor(processor));
                    }
                    log::info!(
                        "🎙️ [DeepFilterNet3] modelo DFN3 listo en {} ms",
                        started.elapsed().as_millis()
                    );
                }
                Ok(Err(e)) => log::warn!("⚠️ [DeepFilterNet3] no pudo cargar el modelo: {:?}", e),
                Err(_) => log::warn!("⚠️ [DeepFilterNet3] panic al inicializar el modelo"),
            }
            LOADING.store(false, Ordering::SeqCst);
        });
    }

    /// Resets the internal STFT/GRU state (e.g., on unmute or mode switch).
    pub fn reset() {
        if let Ok(mut slot) = processor_slot().try_lock() {
            if let Some(processor) = slot.as_mut() {
                let _ = processor.0.init();
            }
        }
    }

    /// Processes a 20 ms capture frame in two 10 ms DFN3 frames, returning the mean
    /// local SNR (dB) if active, or `None` when disabled / still loading.
    pub fn process_capture_lsnr(frame: &mut [f32; 960]) -> Option<f32> {
        if !ENABLED.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(mut slot) = processor_slot().lock() else { return None };
        let Some(processor) = slot.as_mut() else { return None };

        let mut lsnr_sum = 0.0f32;
        let mut lsnr_count = 0u32;
        for chunk in frame.chunks_mut(DFN_FRAME) {
            if chunk.len() != DFN_FRAME {
                continue;
            }
            let Ok(noisy) = ArrayView2::from_shape((1, DFN_FRAME), &chunk[..]) else {
                return None;
            };
            let mut enhanced = [0.0f32; DFN_FRAME];
            let Ok(out) = ArrayViewMut2::from_shape((1, DFN_FRAME), &mut enhanced[..]) else {
                return None;
            };
            match processor.0.process(noisy, out) {
                Ok(lsnr) => {
                    lsnr_sum += lsnr;
                    lsnr_count += 1;
                }
                Err(e) => {
                    log::warn!("⚠️ [DeepFilterNet3] process falló: {:?}", e);
                    return None;
                }
            }
            chunk.copy_from_slice(&enhanced);
        }
        Some(if lsnr_count > 0 { lsnr_sum / lsnr_count as f32 } else { 0.0 })
    }

    /// Processes a 20 ms capture frame in two 10 ms DFN3 frames. Returns false
    /// when the model is disabled or still loading, so the caller can fall back.
    pub fn process_capture(frame: &mut [f32; 960]) -> bool {
        process_capture_lsnr(frame).is_some()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dfn3_loads_and_processes_a_frame() {
            set_enabled(true);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                if processor_slot().lock().map(|p| p.is_some()).unwrap_or(false) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let mut frame = [0.0f32; 960];
            for (i, sample) in frame.iter_mut().enumerate() {
                *sample = (i as f32 * 0.05).sin() * 0.1;
            }
            assert!(process_capture(&mut frame));
            assert!(frame.iter().all(|s| s.is_finite()));
            set_enabled(false);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub fn warm_up() {}

    pub fn reset() {}

    pub fn process_capture_lsnr(_frame: &mut [f32; 960]) -> Option<f32> {
        None
    }

    pub fn process_capture(_frame: &mut [f32; 960]) -> bool {
        false
    }
}

pub use imp::{process_capture, process_capture_lsnr, reset, warm_up};
