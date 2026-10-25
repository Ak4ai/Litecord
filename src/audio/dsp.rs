use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use log::debug;

pub const FRAME_SIZE_SAMPLES: usize = 960; // 20ms at 48kHz
pub const DENOISE_CHUNK_SIZE: usize = 480;  // 10ms at 48kHz (nnnoiseless frame)

// 65536 samples at 48kHz = ~1.365 seconds of audio buffer (power of 2)
const AEC_RING_BUFFER_CAPACITY: usize = 65536;
const AEC_RING_BUFFER_MASK: usize = AEC_RING_BUFFER_CAPACITY - 1;

/// Cache-line aligned AtomicUsize to prevent false sharing between concurrent CPAL audio threads.
/// On multi-core x86/ARM CPUs, separate cache lines ensure the capture thread (reader)
/// and playback thread (writer) do not constantly invalidate each other's L1/L2 caches.
#[repr(align(64))]
pub struct CachePaddedAtomicUsize {
    val: AtomicUsize,
}

impl CachePaddedAtomicUsize {
    pub const fn new(v: usize) -> Self {
        Self {
            val: AtomicUsize::new(v),
        }
    }
    #[inline(always)]
    pub fn load(&self, order: Ordering) -> usize {
        self.val.load(order)
    }
    #[inline(always)]
    pub fn store(&self, v: usize, order: Ordering) {
        self.val.store(v, order);
    }
}

/// Lock-free, zero-allocation SPSC (Single-Producer Single-Consumer) ring buffer for real-time audio.
/// Guarantees that neither playback nor capture CPAL callbacks block on a mutex or trigger heap allocations.
pub struct LockFreeAudioRingBuffer {
    buffer: Box<[AtomicU32]>,
    write_idx: CachePaddedAtomicUsize,
    read_idx: CachePaddedAtomicUsize,
}

impl LockFreeAudioRingBuffer {
    pub fn new() -> Self {
        let mut v = Vec::with_capacity(AEC_RING_BUFFER_CAPACITY);
        for _ in 0..AEC_RING_BUFFER_CAPACITY {
            v.push(AtomicU32::new(0));
        }
        Self {
            buffer: v.into_boxed_slice(),
            write_idx: CachePaddedAtomicUsize::new(0),
            read_idx: CachePaddedAtomicUsize::new(0),
        }
    }

    /// Push audio reference samples from the speaker output callback (Producer).
    /// Lock-free, wait-free, and allocation-free.
    pub fn push_samples(&self, samples: &[f32]) {
        let mut w = self.write_idx.load(Ordering::Relaxed);
        let r = self.read_idx.load(Ordering::Acquire);

        for &s in samples {
            let clamped = s.clamp(-1.0, 1.0);
            self.buffer[w & AEC_RING_BUFFER_MASK].store(clamped.to_bits(), Ordering::Relaxed);
            w = w.wrapping_add(1);
        }

        // Drop oldest unread samples if writer has wrapped around further than capacity
        let current_diff = w.wrapping_sub(r);
        if current_diff > AEC_RING_BUFFER_CAPACITY {
            let new_r = w.wrapping_sub(AEC_RING_BUFFER_CAPACITY);
            self.read_idx.store(new_r, Ordering::Release);
        }

        self.write_idx.store(w, Ordering::Release);
    }

    /// Read audio reference samples in the microphone capture callback (Consumer).
    /// Lock-free, wait-free, and allocation-free.
    ///
    /// Anti-Drift Protection: If the speaker output started before mic capture or if a system
    /// hiccup caused playback samples to accumulate, clamping the read cursor to at most 60 ms
    /// (2880 samples) ensures the AEC reference never lags behind the acoustic return.
    pub fn pop_samples(&self, out: &mut [f32]) -> usize {
        let w = self.write_idx.load(Ordering::Acquire);
        let r = self.read_idx.load(Ordering::Relaxed);
        let available = w.wrapping_sub(r);

        // Anti-Drift Protection: If the speaker output started before mic capture or if a system
        // hiccup caused playback samples to accumulate past 40ms (1920 samples),
        // jump directly to the most recent block needed right now (out.len() samples).
        // This leaves 0 residual samples after reading, preventing stationary queue delay.
        const MAX_AEC_LAG_SAMPLES: usize = 1920; // 40ms trigger threshold
        let (r, available) = if available > MAX_AEC_LAG_SAMPLES {
            let target_keep = out.len().min(AEC_RING_BUFFER_CAPACITY);
            let clamped_r = w.wrapping_sub(target_keep);
            (clamped_r, target_keep)
        } else {
            (r, available)
        };

        let count = out.len().min(available);
        let mut cur_r = r;

        for i in 0..count {
            let bits = self.buffer[cur_r & AEC_RING_BUFFER_MASK].load(Ordering::Relaxed);
            out[i] = f32::from_bits(bits);
            cur_r = cur_r.wrapping_add(1);
        }

        self.read_idx.store(cur_r, Ordering::Release);
        count
    }
}

static AEC_SPEAKER_REF: OnceLock<LockFreeAudioRingBuffer> = OnceLock::new();

pub fn get_aec_speaker_ring() -> &'static LockFreeAudioRingBuffer {
    AEC_SPEAKER_REF.get_or_init(LockFreeAudioRingBuffer::new)
}

/// Push played speaker audio samples so AEC knows what to cancel from the microphone.
/// Fully lock-free, zero-allocation, safe for real-time CPAL audio callbacks.
pub fn push_speaker_reference(samples: &[f32]) {
    get_aec_speaker_ring().push_samples(samples);
}

/// Pop speaker reference samples into a pre-allocated stack buffer.
/// Fully lock-free, zero-allocation, safe for real-time CPAL audio callbacks.
pub fn pop_speaker_reference(out: &mut [f32]) -> usize {
    get_aec_speaker_ring().pop_samples(out)
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

/// 2nd-order Butterworth High-Pass Filter (80Hz cutoff at 48kHz)
/// Eliminates microphone desk thumps, low rumble, and electrical hum without affecting vocal formants
#[derive(Debug, Clone, Copy)]
pub struct BiquadHpf {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadHpf {
    pub fn new_80hz_48khz() -> Self {
        // Precomputed Butterworth 2nd-order HPF @ 80Hz, fs=48000Hz (Q = 0.7071)
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            b0: 0.985287,
            b1: -1.970574,
            b2: 0.985287,
            a1: -1.970500,
            a2: 0.970648,
        }
    }

    #[inline(always)]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn process_frame(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        for s in frame.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

/// Complete Voice Processing Pipeline:
/// High-Pass Filter (80Hz) -> Acoustic Echo Cancellation (AEC) -> Noise Suppression (RNNoise) -> AGC -> Soft Limiter
pub struct VoiceProcessor {
    denoiser: Box<nnnoiseless::DenoiseState<'static>>,
    hpf: BiquadHpf,
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
        let mut denoiser = nnnoiseless::DenoiseState::new();
        // Pre-warm the recurrent GRU states of RNNoise with silent frames
        let mut silence = [0.0f32; DENOISE_CHUNK_SIZE];
        let mut dummy_out = [0.0f32; DENOISE_CHUNK_SIZE];
        for _ in 0..5 {
            denoiser.process_frame(&mut dummy_out, &mut silence);
        }

        Self {
            denoiser,
            hpf: BiquadHpf::new_80hz_48khz(),
            aec_filter: [0.0; 48],
            aec_delay_buffer: [0.0; 1024],
            aec_delay_idx: 0,
            agc_envelope: 0.08, // Natural speech level starting envelope (prevents 6x initial surge)
            agc_current_gain: 1.0,
            transient_env: 0.0,
            noise_floor: 0.005,
        }
    }

    /// Process a 20ms (960 samples at 48kHz) frame in place
    pub fn process_frame(
        &mut self,
        frame: &mut [f32; FRAME_SIZE_SAMPLES],
        suppression_mode: &str, // "advanced", "standard", "none" (accepts legacy "krisp")
        enable_aec: bool,
        enable_agc: bool,
    ) {
        // Fast-path bypass if all filters are turned off
        let is_bypass = (suppression_mode == "none" || suppression_mode.is_empty()) && !enable_aec && !enable_agc;
        if is_bypass {
            return;
        }

        // 1. High-Pass Filter (80Hz Butterworth) to eliminate DC offsets and thumps
        self.hpf.process_frame(frame);

        // 2. Acoustic Echo Cancellation (AEC)
        if enable_aec {
            self.apply_aec(frame);
        }

        // 3. Noise Suppression
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

        // 4. Automatic Gain Control (AGC) & Soft Limiter
        if enable_agc {
            self.apply_agc(frame);
        }

        // 5. Hard safety clamp
        for s in frame.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    /// Applies Acoustic Echo Cancellation using a Normalized LMS adaptive filter.
    /// Operates completely lock-free and allocation-free on the CPAL capture thread.
    fn apply_aec(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        let mut speaker_samples = [0.0f32; FRAME_SIZE_SAMPLES];
        let count = pop_speaker_reference(&mut speaker_samples);
        if count == 0 {
            return;
        }

        let mu = 0.015f32; // NLMS step size
        let num_taps = self.aec_filter.len();

        for i in 0..frame.len() {
            let ref_sample = speaker_samples[i];
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
                // Apply residual echo reduction when speaker is actively playing
                frame[i] = error;
            }

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

    /// Applies advanced neural denoising with transparent transient click suppression
    fn apply_advanced_suppression(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        // Step 1: Run recurrent neural denoiser
        self.apply_standard_rnnoise(frame);

        // Step 2: Transient Click & Tap Suppression (keyboards, mouse clicks)
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
                // Smoothly compress transient click without waveshaping speech fundamentals
                let suppression = 1.0 / (1.0 + (crest_ratio - 18.0) * 0.25);
                *s *= suppression;
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
            (target_rms / self.agc_envelope).clamp(0.6, 2.5)
        } else {
            1.0
        };

        // Smooth gain transition
        self.agc_current_gain = self.agc_current_gain * 0.97 + desired_gain * 0.03;

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
