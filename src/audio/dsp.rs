use std::sync::{Arc, Mutex, OnceLock};
use std::collections::VecDeque;
use log::debug;

pub const FRAME_SIZE_SAMPLES: usize = 960; // 20ms at 48kHz
pub const DENOISE_CHUNK_SIZE: usize = 480;  // 10ms at 48kHz (nnnoiseless frame)

/// Global speaker loopback queue for Acoustic Echo Cancellation (AEC)
static AEC_SPEAKER_REF: OnceLock<Arc<Mutex<VecDeque<f32>>>> = OnceLock::new();

pub fn get_aec_speaker_queue() -> Arc<Mutex<VecDeque<f32>>> {
    AEC_SPEAKER_REF.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(96000)))).clone()
}

/// Push played speaker audio samples so AEC knows what to cancel from the microphone
pub fn push_speaker_reference(samples: &[f32]) {
    if let Ok(mut q) = get_aec_speaker_queue().lock() {
        if q.len() > 48000 {
            // Keep at most ~1 second of playback history
            let excess = q.len() - 24000;
            q.drain(0..excess);
        }
        for &s in samples {
            q.push_back(s.clamp(-1.0, 1.0));
        }
    }
}

/// Fractional phase resampler that prevents micro-stutter and audio drift
#[derive(Debug, Default)]
pub struct PhaseResampler {
    phase: f64,
    last_sample: f32,
}

impl PhaseResampler {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            last_sample: 0.0,
        }
    }

    pub fn resample(&mut self, input: &[f32], src_rate: u32, dst_rate: u32, output: &mut Vec<f32>) {
        if src_rate == dst_rate || src_rate == 0 {
            output.extend_from_slice(input);
            return;
        }

        let ratio = dst_rate as f64 / src_rate as f64;
        let step = 1.0 / ratio;

        let mut curr_phase = self.phase;
        let in_len = input.len();

        while curr_phase < in_len as f64 {
            let idx = curr_phase as usize;
            let frac = (curr_phase - idx as f64) as f32;

            let s0 = if idx == 0 {
                self.last_sample
            } else {
                input[idx - 1]
            };
            let s1 = input[idx];

            let sample = s0 * (1.0 - frac) + s1 * frac;
            output.push(sample.clamp(-1.0, 1.0));

            curr_phase += step;
        }

        self.phase = curr_phase - in_len as f64;
        if let Some(&last) = input.last() {
            self.last_sample = last;
        }
    }
}

/// Complete Voice Processing Pipeline:
/// Acoustic Echo Cancellation (AEC) -> Noise Suppression (Krisp / Standard / None) -> AGC -> Soft Limiter
pub struct VoiceProcessor {
    denoiser: Box<nnnoiseless::DenoiseState<'static>>,
    // Echo cancellation adaptive filter coefficients
    aec_filter: [f32; 48],
    aec_delay_buffer: [f32; 1024],
    aec_delay_idx: usize,
    // Automatic Gain Control state
    agc_envelope: f32,
    agc_current_gain: f32,
    // Transient suppression state
    transient_env: f32,
    noise_floor: f32,
}

impl VoiceProcessor {
    pub fn new() -> Self {
        Self {
            denoiser: nnnoiseless::DenoiseState::new(),
            aec_filter: [0.0; 48],
            aec_delay_buffer: [0.0; 1024],
            aec_delay_idx: 0,
            agc_envelope: 0.02,
            agc_current_gain: 1.0,
            transient_env: 0.0,
            noise_floor: 0.005,
        }
    }

    /// Process a 20ms (960 samples at 48kHz) frame in place
    pub fn process_frame(
        &mut self,
        frame: &mut [f32; FRAME_SIZE_SAMPLES],
        suppression_mode: &str, // "advanced", "standard", "none" (also accepts legacy "krisp")
        enable_aec: bool,
        enable_agc: bool,
    ) {
        // 1. Acoustic Echo Cancellation (AEC)
        if enable_aec {
            self.apply_aec(frame);
        }

        // 2. Noise Suppression
        match suppression_mode {
            "advanced" | "krisp" => {
                self.apply_advanced_suppression(frame);
            }
            "standard" => {
                self.apply_standard_rnnoise(frame);
            }
            _ => {
                // "none" or bypass
            }
        }

        // 3. Automatic Gain Control (AGC) & Soft Limiter
        if enable_agc {
            self.apply_agc(frame);
        }

        // 4. Hard safety clamp
        for s in frame.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    /// Applies Acoustic Echo Cancellation using a Normalized LMS adaptive filter
    fn apply_aec(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        let speaker_samples: Vec<f32> = if let Ok(mut q) = get_aec_speaker_queue().lock() {
            let count = frame.len().min(q.len());
            let mut buf = Vec::with_capacity(count);
            for _ in 0..count {
                if let Some(s) = q.pop_front() {
                    buf.push(s);
                }
            }
            buf
        } else {
            Vec::new()
        };

        if speaker_samples.is_empty() {
            return;
        }

        let mu = 0.015f32; // NLMS step size
        let num_taps = self.aec_filter.len();

        for i in 0..frame.len() {
            let ref_sample = if i < speaker_samples.len() {
                speaker_samples[i]
            } else {
                0.0
            };

            self.aec_delay_buffer[self.aec_delay_idx] = ref_sample;

            // Compute estimated echo
            let mut echo_estimate = 0.0f32;
            let mut power = 1e-4f32;
            for tap in 0..num_taps {
                let delay_pos = (self.aec_delay_idx + self.aec_delay_buffer.len() - tap) % self.aec_delay_buffer.len();
                let x = self.aec_delay_buffer[delay_pos];
                echo_estimate += self.aec_filter[tap] * x;
                power += x * x;
            }

            // Error signal (mic minus echo)
            let mic_val = frame[i];
            let error = mic_val - echo_estimate;

            // Only adapt filter when there is significant speaker playback
            if power > 0.001 {
                let norm = mu / power;
                for tap in 0..num_taps {
                    let delay_pos = (self.aec_delay_idx + self.aec_delay_buffer.len() - tap) % self.aec_delay_buffer.len();
                    self.aec_filter[tap] += norm * error * self.aec_delay_buffer[delay_pos];
                }
            }

            // Apply residual echo suppression
            frame[i] = error;

            self.aec_delay_idx = (self.aec_delay_idx + 1) % self.aec_delay_buffer.len();
        }
    }

    /// Applies standard RNNoise neural network noise suppression
    fn apply_standard_rnnoise(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        let mut chunk_out = [0.0f32; DENOISE_CHUNK_SIZE];

        // First 480 samples
        self.denoiser.process_frame(&mut chunk_out, &frame[0..DENOISE_CHUNK_SIZE]);
        frame[0..DENOISE_CHUNK_SIZE].copy_from_slice(&chunk_out);

        // Second 480 samples
        self.denoiser.process_frame(&mut chunk_out, &frame[DENOISE_CHUNK_SIZE..FRAME_SIZE_SAMPLES]);
        frame[DENOISE_CHUNK_SIZE..FRAME_SIZE_SAMPLES].copy_from_slice(&chunk_out);
    }

    /// Applies advanced neural denoising with transient click suppression & spectral gate
    fn apply_advanced_suppression(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        // Step 1: Run recurrent neural denoiser
        self.apply_standard_rnnoise(frame);

        // Step 2: Advanced Transient Click & Tap Suppression (keyboards, mouse clicks)
        let alpha_attack = 0.35f32;
        let alpha_decay = 0.008f32;

        for s in frame.iter_mut() {
            let abs_val = s.abs();

            // Track fast transient envelope vs smooth noise floor
            if abs_val > self.transient_env {
                self.transient_env = (1.0 - alpha_attack) * self.transient_env + alpha_attack * abs_val;
            } else {
                self.transient_env = (1.0 - alpha_decay) * self.transient_env + alpha_decay * abs_val;
            }

            self.noise_floor = self.noise_floor * 0.999 + abs_val * 0.001;

            // Transient detection: sudden high peak with fast decay that isn't sustained vocal energy
            let crest_ratio = self.transient_env / (self.noise_floor + 1e-4);
            if crest_ratio > 18.0 && abs_val > 0.12 {
                // Smoothly compress transient click
                let suppression = 1.0 / (1.0 + (crest_ratio - 18.0) * 0.25);
                *s *= suppression;
            }

            // Downward expander for very low level background noise (< -42 dBFS)
            if abs_val < 0.008 {
                *s *= (abs_val / 0.008).powf(1.6);
            }
        }
    }

    /// Automatic Gain Control (AGC) with smooth speech targeting and transparent soft knee
    fn apply_agc(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        // Calculate frame RMS
        let mut sum_sq = 0.0f32;
        for &s in frame.iter() {
            sum_sq += s * s;
        }
        let frame_rms = (sum_sq / frame.len() as f32).sqrt();

        // Smoothly track voice envelope (ignore absolute silence)
        if frame_rms > 0.005 {
            let attack = 0.08f32;
            let release = 0.015f32;
            if frame_rms > self.agc_envelope {
                self.agc_envelope = (1.0 - attack) * self.agc_envelope + attack * frame_rms;
            } else {
                self.agc_envelope = (1.0 - release) * self.agc_envelope + release * frame_rms;
            }
        }

        // Target speech level is ~ -16 dBFS (approx 0.16 RMS)
        let target_rms = 0.16f32;
        let desired_gain = if self.agc_envelope > 0.008 {
            (target_rms / self.agc_envelope).clamp(0.5, 3.5)
        } else {
            1.0
        };

        // Smooth gain transition
        self.agc_current_gain = self.agc_current_gain * 0.96 + desired_gain * 0.04;

        // Apply gain with tanh-based soft-knee saturation
        for s in frame.iter_mut() {
            let amplified = *s * self.agc_current_gain;
            if amplified.abs() > 0.75 {
                // Soft saturation curve: prevents harsh digital clipping
                let sign = amplified.signum();
                let x = amplified.abs() - 0.75;
                let saturated = 0.75 + (0.25 * (x / 0.25).tanh());
                *s = sign * saturated;
            } else {
                *s = amplified;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_phase_resampler_clean_output() {
        let mut resampler = PhaseResampler::new();
        let input = vec![0.5f32; 882]; // 20ms of 44.1kHz
        let mut output = Vec::new();
        resampler.resample(&input, 44100, 48000, &mut output);
        // Expect ~960 samples
        assert!((output.len() as i32 - 960).abs() <= 2);
    }

    #[test]
    fn test_voice_processor_execution_cost() {
        let mut vp = VoiceProcessor::new();
        let mut frame = [0.1f32; FRAME_SIZE_SAMPLES];

        // 1. Benchmark: None (bypass)
        let iters = 100;
        let start = Instant::now();
        for _ in 0..iters {
            vp.process_frame(&mut frame, "none", false, false);
        }
        let elapsed_none_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 2. Benchmark: Standard (RNNoise only)
        let start = Instant::now();
        for _ in 0..iters {
            vp.process_frame(&mut frame, "standard", false, false);
        }
        let elapsed_std_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 3. Benchmark: Advanced (Neural + Transients)
        let start = Instant::now();
        for _ in 0..iters {
            vp.process_frame(&mut frame, "advanced", false, false);
        }
        let elapsed_adv_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 4. Benchmark: All enabled (Advanced + AEC + AGC)
        let start = Instant::now();
        for _ in 0..iters {
            vp.process_frame(&mut frame, "advanced", true, true);
        }
        let elapsed_all_us = start.elapsed().as_micros() as f64 / (iters as f64);

        println!(
            "\n=== Voice Processing Benchmark (20ms / 960-sample frame) ===\n\
             Budget per frame: 20,000 µs (20.0 ms)\n\
             - Bypass / None:       {:.2} µs ({:.4}% of budget)\n\
             - Standard RNNoise:    {:.2} µs ({:.4}% of budget)\n\
             - Advanced Neural:     {:.2} µs ({:.4}% of budget)\n\
             - Full (Adv+AEC+AGC):  {:.2} µs ({:.4}% of budget)\n",
            elapsed_none_us, (elapsed_none_us / 20000.0) * 100.0,
            elapsed_std_us, (elapsed_std_us / 20000.0) * 100.0,
            elapsed_adv_us, (elapsed_adv_us / 20000.0) * 100.0,
            elapsed_all_us, (elapsed_all_us / 20000.0) * 100.0
        );

        // Assert that even full processing takes well under 10% of the 20ms frame budget (2000 µs = 2ms)
        assert!(elapsed_all_us < 2000.0, "Voice processing should take < 2ms per 20ms frame");
    }
}
