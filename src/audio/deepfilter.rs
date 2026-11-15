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
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        imp::warm_up();
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
                    &df::tract::RuntimeParams::default_with_ch(1),
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

    /// Processes a 20 ms capture frame in two 10 ms DFN3 frames. Returns false
    /// when the model is disabled or still loading, so the caller can fall back.
    pub fn process_capture(frame: &mut [f32; 960]) -> bool {
        if !ENABLED.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(mut slot) = processor_slot().lock() else { return false };
        let Some(processor) = slot.as_mut() else { return false };

        for chunk in frame.chunks_mut(DFN_FRAME) {
            if chunk.len() != DFN_FRAME {
                continue;
            }
            let Ok(noisy) = ArrayView2::from_shape((1, DFN_FRAME), &chunk[..]) else {
                return false;
            };
            let mut enhanced = [0.0f32; DFN_FRAME];
            let Ok(out) = ArrayViewMut2::from_shape((1, DFN_FRAME), &mut enhanced[..]) else {
                return false;
            };
            if let Err(e) = processor.0.process(noisy, out) {
                log::warn!("⚠️ [DeepFilterNet3] process falló: {:?}", e);
                return false;
            }
            chunk.copy_from_slice(&enhanced);
        }
        true
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

    pub fn process_capture(_frame: &mut [f32; 960]) -> bool {
        false
    }
}

pub use imp::{process_capture, warm_up};
