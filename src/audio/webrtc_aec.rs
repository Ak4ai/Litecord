//! WebRTC AEC3 echo cancellation for voice capture (Linux).
//!
//! Discord's client is built on the WebRTC AudioProcessing module; this wires
//! the same AEC3 engine into Litecord's render (speaker) and capture (mic)
//! paths. The custom NLMS AEC in `dsp.rs` stays as a fallback for platforms
//! where this module is not compiled, or if AEC3 fails to initialize.
//!
//! The engine works on 10 ms frames at 48 kHz. Litecord's capture path works
//! on 20 ms frames, so they are split here. Render frames are fed directly from
//! the speaker callback (mono downmix), resampling device rates when needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// 10 ms at 48 kHz, the frame size required by WebRTC AudioProcessing.
const APM_FRAME: usize = 480;

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        // Initialize the C++ AEC3 processor off the real-time audio callback.
        imp::warm_up();
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use crate::audio::dsp::PhaseResampler;
    use webrtc_audio_processing::config::{Config, EchoCanceller};
    use webrtc_audio_processing::Processor;

    static PROCESSOR: OnceLock<Option<Processor>> = OnceLock::new();

    fn processor() -> Option<&'static Processor> {
        PROCESSOR
            .get_or_init(|| {
                let processor = Processor::new(48_000).map_err(|e| {
                    log::warn!("⚠️ [AEC3] WebRTC AudioProcessing no pudo iniciarse: {:?}", e);
                }).ok()?;
                processor.set_config(Config {
                    // Full AEC3; stream_delay_ms = None lets it estimate the
                    // acoustic delay between the speaker and the microphone.
                    echo_canceller: Some(EchoCanceller::Full { stream_delay_ms: None }),
                    // AEC3 needs a high-pass stage; the custom DSP still applies
                    // its own 80 Hz filter afterwards, which is harmless.
                    high_pass_filter: Some(Default::default()),
                    // Noise suppression and AGC stay on Litecord's pipeline.
                    noise_suppression: None,
                    gain_controller: None,
                    ..Default::default()
                });
                log::info!("🎙️ [AEC3] Cancelación de eco WebRTC AEC3 activa (48 kHz).");
                Some(processor)
            })
            .as_ref()
    }

    pub fn warm_up() {
        let _ = processor();
    }

    struct RenderState {
        resampler: PhaseResampler,
        input_rate: u32,
        pending: Vec<f32>,
        scratch: Vec<f32>,
    }

    impl RenderState {
        fn new() -> Self {
            Self {
                resampler: PhaseResampler::new(),
                input_rate: 0,
                pending: Vec::with_capacity(APM_FRAME * 4),
                scratch: Vec::with_capacity(2048),
            }
        }

        fn push(&mut self, processor: &Processor, samples: &[f32], sample_rate: u32) {
            if samples.is_empty() || sample_rate == 0 {
                return;
            }
            if sample_rate != self.input_rate {
                self.input_rate = sample_rate;
                self.resampler = PhaseResampler::new();
                self.pending.clear();
            }

            if sample_rate == 48_000 {
                self.pending.extend_from_slice(samples);
            } else {
                self.scratch.clear();
                self.scratch.reserve(samples.len() * 48_000 / sample_rate as usize + 32);
                self.resampler
                    .resample(samples, sample_rate, 48_000, &mut self.scratch);
                self.pending.extend_from_slice(&self.scratch);
            }

            let mut offset = 0;
            while self.pending.len() - offset >= APM_FRAME {
                let mut frame = [0.0f32; APM_FRAME];
                frame.copy_from_slice(&self.pending[offset..offset + APM_FRAME]);
                if let Err(e) = processor.process_render_frame([&mut frame]) {
                    log::debug!("[AEC3] process_render_frame: {:?}", e);
                    break;
                }
                offset += APM_FRAME;
            }
            if offset > 0 {
                self.pending.drain(..offset);
            }
        }
    }

    thread_local! {
        static RENDER: std::cell::RefCell<RenderState> = std::cell::RefCell::new(RenderState::new());
    }

    pub fn push_render(samples: &[f32], sample_rate: u32) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(processor) = processor() else { return };
        RENDER.with(|state| state.borrow_mut().push(processor, samples, sample_rate));
    }

    /// Runs AEC3 over a 20 ms capture frame. Returns false when AEC3 is not
    /// available/enabled, so the caller can fall back to the custom AEC.
    pub fn process_capture(frame: &mut [f32; 960]) -> bool {
        if !ENABLED.load(Ordering::Relaxed) {
            return false;
        }
        let Some(processor) = processor() else { return false };
        let mut processed = false;
        for chunk in frame.chunks_mut(APM_FRAME) {
            if chunk.len() != APM_FRAME {
                continue;
            }
            match processor.process_capture_frame([chunk]) {
                Ok(()) => processed = true,
                Err(e) => {
                    log::warn!("⚠️ [AEC3] process_capture_frame falló: {:?}", e);
                    return false;
                }
            }
        }
        processed
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn aec3_processes_a_render_and_capture_pair() {
            set_enabled(true);
            let mut render = [0.0f32; APM_FRAME];
            let mut capture = [0.0f32; 960];
            for i in 0..APM_FRAME {
                let t = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                render[i] = t;
                capture[i] = t * 0.2;
                capture[APM_FRAME + i] = t * 0.2;
            }
            push_render(&render, 48_000);
            assert!(process_capture(&mut capture));
            assert!(capture.iter().all(|s| s.is_finite()));
            set_enabled(false);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub fn warm_up() {}

    pub fn push_render(_samples: &[f32], _sample_rate: u32) {}

    pub fn process_capture(_frame: &mut [f32; 960]) -> bool {
        false
    }
}

pub use imp::{process_capture, push_render};
