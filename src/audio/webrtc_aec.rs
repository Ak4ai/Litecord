//! WebRTC AudioProcessing integration (Linux).
//!
//! Discord's baseline voice pipeline (per its public SDK documentation) is the
//! WebRTC AudioProcessing module: AEC3 echo cancellation, WebRTC noise
//! suppression and AGC. This module exposes that same open-source stack in
//! Litecord. The proprietary Krisp model Discord optionally ships is not
//! reproduced here; the "Discord (WebRTC)" mode is the WebRTC baseline, not
//! Krisp.
//!
//! The engine works on 10 ms frames at 48 kHz. Litecord's capture path works
//! on 20 ms frames, so they are split here. Render frames are fed directly from
//! the speaker callback (mono downmix), resampling device rates when needed.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// 10 ms at 48 kHz, the frame size required by WebRTC AudioProcessing.
const APM_FRAME: usize = 480;

const MODULE_AEC: u8 = 1 << 0;
const MODULE_NS: u8 = 1 << 1;
const MODULE_AGC: u8 = 1 << 2;

static MODULES: AtomicU8 = AtomicU8::new(0);

/// Which WebRTC modules actually processed a capture frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    pub aec: bool,
    pub ns: bool,
    pub agc: bool,
}

impl Applied {
    fn from_modules(modules: u8) -> Self {
        Self {
            aec: modules & MODULE_AEC != 0,
            ns: modules & MODULE_NS != 0,
            agc: modules & MODULE_AGC != 0,
        }
    }
}

/// Enables the WebRTC submodules. `ns` maps to Litecord's "Discord (WebRTC)"
/// noise mode; `agc` follows the automatic-gain toggle.
pub fn configure(aec: bool, ns: bool, agc: bool) {
    let mut modules = 0u8;
    if aec {
        modules |= MODULE_AEC;
    }
    if ns {
        modules |= MODULE_NS;
    }
    if agc {
        modules |= MODULE_AGC;
    }
    MODULES.store(modules, Ordering::SeqCst);
    if modules != 0 {
        // Initialize the C++ processor off the real-time audio callback.
        imp::warm_up();
        imp::apply_config();
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use crate::audio::dsp::PhaseResampler;
    use webrtc_audio_processing::config::{
        Config, EchoCanceller, GainController, GainController2, NoiseSuppression,
        NoiseSuppressionLevel,
    };
    use webrtc_audio_processing::Processor;

    static PROCESSOR: OnceLock<Option<Processor>> = OnceLock::new();

    fn build_config() -> Config {
        let modules = MODULES.load(Ordering::Relaxed);
        let mut config = Config {
            high_pass_filter: Some(Default::default()),
            ..Default::default()
        };
        if modules & MODULE_AEC != 0 {
            config.echo_canceller = Some(EchoCanceller::Full { stream_delay_ms: None });
        }
        if modules & MODULE_NS != 0 {
            config.noise_suppression = Some(NoiseSuppression {
                level: NoiseSuppressionLevel::High,
                analyze_linear_aec_output: false,
            });
        }
        if modules & MODULE_AGC != 0 {
            config.gain_controller = Some(GainController::GainController2(GainController2 {
                input_volume_controller_enabled: false,
                adaptive_digital: Some(Default::default()),
                fixed_digital: Default::default(),
            }));
        }
        config
    }

    fn processor() -> Option<&'static Processor> {
        PROCESSOR
            .get_or_init(|| {
                let processor = Processor::new(48_000)
                    .map_err(|e| {
                        log::warn!("⚠️ [WebRTC APM] no pudo iniciarse: {:?}", e);
                    })
                    .ok()?;
                processor.set_config(build_config());
                log::info!("🎙️ [WebRTC APM] AEC3/NS/AGC activos (48 kHz).");
                Some(processor)
            })
            .as_ref()
    }

    pub fn warm_up() {
        let _ = processor();
    }

    pub fn apply_config() {
        if let Some(Some(processor)) = PROCESSOR.get() {
            processor.set_config(build_config());
        }
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
                    log::debug!("[WebRTC APM] process_render_frame: {:?}", e);
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
        // Only AEC3 consumes the render reference.
        if MODULES.load(Ordering::Relaxed) & MODULE_AEC == 0 {
            return;
        }
        let Some(processor) = processor() else { return };
        RENDER.with(|state| state.borrow_mut().push(processor, samples, sample_rate));
    }

    /// Runs the enabled WebRTC modules over a 20 ms capture frame.
    pub fn process_capture(frame: &mut [f32; 960]) -> Applied {
        let modules = MODULES.load(Ordering::Relaxed);
        if modules == 0 {
            return Applied::default();
        }
        let Some(processor) = processor() else {
            return Applied::default();
        };
        for chunk in frame.chunks_mut(APM_FRAME) {
            if chunk.len() != APM_FRAME {
                continue;
            }
            if let Err(e) = processor.process_capture_frame([chunk]) {
                log::warn!("⚠️ [WebRTC APM] process_capture_frame falló: {:?}", e);
                return Applied::default();
            }
        }
        Applied::from_modules(modules)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn webrtc_processes_a_render_and_capture_pair() {
            configure(true, true, false);
            let mut render = [0.0f32; APM_FRAME];
            let mut capture = [0.0f32; 960];
            for i in 0..APM_FRAME {
                let t = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                render[i] = t;
                capture[i] = t * 0.2;
                capture[APM_FRAME + i] = t * 0.2;
            }
            push_render(&render, 48_000);
            let applied = process_capture(&mut capture);
            assert!(applied.aec);
            assert!(applied.ns);
            assert!(capture.iter().all(|s| s.is_finite()));
            configure(false, false, false);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub fn warm_up() {}

    pub fn apply_config() {}

    pub fn push_render(_samples: &[f32], _sample_rate: u32) {}

    pub fn process_capture(_frame: &mut [f32; 960]) -> Applied {
        Applied::default()
    }
}

pub use imp::{apply_config, process_capture, push_render, warm_up};
