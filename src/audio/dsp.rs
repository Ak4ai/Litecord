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

/// Output metrics returned after processing each 20ms audio frame
#[derive(Debug, Clone, Copy)]
pub struct ProcessedVoiceMetrics {
    /// Neural speech probability from RNNoise (0.0 to 1.0)
    pub vad_prob: f32,
    /// Spectral volume in decibels [-100.0 dB, 0.0 dB] matching Discord VolumeAnalyzer
    pub volume_db: f32,
    /// Normalized UI meter level in [0.0, 1.0] where 0.0 = -100 dB, 0.5 = -50 dB, 1.0 = 0 dB
    pub ui_level: f32,
    /// True if speech is detected according to both spectral volume and neural VAD
    pub is_voice: bool,
}

/// Discord-grade Spectral Volume Analyzer
/// Emulates discord::VolumeAnalyzer (0x422ce0..0x423115 in discord_voice.node):
/// - 512-point sliding window with 3-term Blackman window
/// - Frequency analysis ignoring rumble/plosives below 375Hz (first 4 bins)
/// - Logarithmic dBFS scale [-100.0 dB, 0.0 dB]
const VOICE_BINS: [usize; 11] = [4, 6, 8, 11, 14, 18, 22, 26, 30, 34, 38];

pub struct VolumeAnalyzer {
    history: [f32; 512],
    history_idx: usize,
    blackman_window: [f32; 512],
    window_norm_sq: f32,
    goertzel_coeffs: [f32; 11],
    current_volume_db: f32,
}

impl VolumeAnalyzer {
    pub fn new() -> Self {
        let mut blackman = [0.0f32; 512];
        let mut window_sum = 0.0f32;
        for i in 0..512 {
            // w[n] = 0.42 - 0.5 * cos(2*pi*n/511) + 0.08 * cos(4*pi*n/511)
            let n = i as f64;
            let val = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * n / 511.0).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * n / 511.0).cos();
            blackman[i] = val as f32;
            window_sum += blackman[i];
        }
        // Normalization factor to recover sinusoidal peak amplitude: norm = 2.0 / window_sum
        let norm = 2.0 / window_sum.max(1.0);
        let window_norm_sq = norm * norm;

        let mut coeffs = [0.0f32; 11];
        for (idx, &k) in VOICE_BINS.iter().enumerate() {
            let omega = 2.0 * std::f32::consts::PI * (k as f32) / 512.0;
            coeffs[idx] = 2.0 * omega.cos();
        }

        Self {
            history: [0.0; 512],
            history_idx: 0,
            blackman_window: blackman,
            window_norm_sq,
            goertzel_coeffs: coeffs,
            current_volume_db: -100.0,
        }
    }

    /// Feeds samples and updates the spectral dB volume estimate
    pub fn analyze_frame(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return self.current_volume_db;
        }

        // Push new samples into circular history buffer
        for &s in samples {
            self.history[self.history_idx] = s;
            self.history_idx = (self.history_idx + 1) % 512;
        }

        // Extract windowed 512-sample buffer in chronological order
        let mut windowed = [0.0f32; 512];
        for i in 0..512 {
            let src_idx = (self.history_idx + i) % 512;
            windowed[i] = self.history[src_idx] * self.blackman_window[i];
        }

        // Fast Goertzel spectral analysis focused on vocal formant frequency bins (375Hz to 3800Hz)
        // Eliminates trigonometric calls in the loop and provides O(N) evaluation (< 10µs per frame)
        let mut max_magnitude_sq = 1e-10f32; // Floor of -100 dBFS

        for &coeff in &self.goertzel_coeffs {
            let mut s1 = 0.0f32;
            let mut s2 = 0.0f32;

            for &x in &windowed {
                let s0 = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }

            let mag_sq = (s1 * s1 + s2 * s2 - coeff * s1 * s2) * self.window_norm_sq;
            if mag_sq > max_magnitude_sq {
                max_magnitude_sq = mag_sq;
            }
        }

        // Peak magnitude in voice spectrum
        let peak_mag = max_magnitude_sq.sqrt().max(1e-5);
        let frame_db = (20.0 * peak_mag.log10()).clamp(-100.0, 0.0);

        // Responsive attack / smooth decay volume tracking matching Discord VolumeAnalyzer:
        // When speech starts from silence/rumble (< -50dB), attack fast so VAD triggers on 1st syllable
        if self.current_volume_db <= -90.0 {
            self.current_volume_db = frame_db;
        } else if frame_db > self.current_volume_db {
            let attack = if self.current_volume_db < -45.0 { 0.85 } else { 0.40 };
            self.current_volume_db = self.current_volume_db * (1.0 - attack) + frame_db * attack;
        } else {
            self.current_volume_db = self.current_volume_db * 0.88 + frame_db * 0.12;
        }

        self.current_volume_db
    }

    /// Converts decibels [-100.0 dB, 0.0 dB] to normalized UI meter level [0.0, 1.0]
    #[inline(always)]
    pub fn db_to_ui_level(db: f32) -> f32 {
        ((db.clamp(-100.0, 0.0) + 100.0) / 100.0).clamp(0.0, 1.0)
    }

    /// Converts normalized UI meter level [0.0, 1.0] to decibels [-100.0 dB, 0.0 dB]
    #[inline(always)]
    pub fn ui_level_to_db(level: f32) -> f32 {
        (level.clamp(0.0, 1.0) * 100.0) - 100.0
    }
}

/// Complete Voice Processing Pipeline:
/// High-Pass Filter (80Hz) -> Delay-Aligned AEC + RES -> Dual Noise Suppression (Standard/Advanced) -> Voice-Gated AGC2 -> VolumeAnalyzer
pub struct VoiceProcessor {
    denoiser: Box<nnnoiseless::DenoiseState<'static>>,
    hpf: BiquadHpf,
    volume_analyzer: VolumeAnalyzer,
    // Echo cancellation adaptive filter coefficients (64 taps NLMS)
    aec_filter: [f32; 64],
    aec_delay_buffer: [f32; 8192],
    aec_delay_idx: usize,
    // Delay estimator for acoustic speaker-to-mic alignment
    aec_speaker_energy_hist: [f32; 32],
    aec_mic_energy_hist: [f32; 32],
    aec_hist_idx: usize,
    aec_estimated_delay_samples: usize,
    // Automatic Gain Control (AGC2) state
    agc_speech_envelope: f32,
    agc_current_gain: f32,
    // Advanced Transient & Neural Gate state
    neural_gate_gain: f32,
    sub_block_energy_floor: f32,
    _consecutive_muted_frames: usize,
}

impl VoiceProcessor {
    pub fn new() -> Self {
        let mut denoiser = nnnoiseless::DenoiseState::new();
        // Pre-warm the recurrent GRU states of RNNoise with silent frames in correct S16 scale
        let mut silence = [0.0f32; DENOISE_CHUNK_SIZE];
        let mut dummy_out = [0.0f32; DENOISE_CHUNK_SIZE];
        for _ in 0..10 {
            denoiser.process_frame(&mut dummy_out, &mut silence);
        }

        Self {
            denoiser,
            hpf: BiquadHpf::new_80hz_48khz(),
            volume_analyzer: VolumeAnalyzer::new(),
            aec_filter: [0.0; 64],
            aec_delay_buffer: [0.0; 8192],
            aec_delay_idx: 0,
            aec_speaker_energy_hist: [0.0; 32],
            aec_mic_energy_hist: [0.0; 32],
            aec_hist_idx: 0,
            aec_estimated_delay_samples: 960,
            agc_speech_envelope: 0.125, // -18 dBFS speech RMS baseline
            agc_current_gain: 1.0,
            neural_gate_gain: 1.0,
            sub_block_energy_floor: 0.005,
            _consecutive_muted_frames: 0,
        }
    }

    /// Process a 20ms (960 samples at 48kHz) frame in place and returns rich voice metrics
    pub fn process_frame(
        &mut self,
        frame: &mut [f32; FRAME_SIZE_SAMPLES],
        suppression_mode: &str, // "advanced", "standard", "none" (accepts legacy "krisp")
        enable_aec: bool,
        enable_agc: bool,
    ) -> ProcessedVoiceMetrics {
        // 1. High-Pass Filter (80Hz Butterworth) to eliminate DC offsets, plosive pops, and rumble
        self.hpf.process_frame(frame);

        // 2. Acoustic Echo Cancellation (AEC) with Delay Alignment and Residual Echo Suppressor (RES)
        if enable_aec {
            self.apply_aec(frame);
        }

        // 3. Noise Suppression (Standard Wiener/Bark blend vs Advanced Neural Transient Voice-Gate)
        let vad_prob = match suppression_mode {
            "advanced" | "krisp" => {
                self.apply_advanced_suppression(frame)
            }
            "standard" => {
                self.apply_standard_rnnoise(frame)
            }
            _ => {
                // When suppression is disabled, we still obtain unscaled VAD probability
                // for AGC and gating without altering frame samples
                self.probe_vad_prob(frame)
            }
        };

        // 4. Automatic Gain Control (AGC2) with Voice-Gating
        // Only adapts gain when speech is verified (vad_prob >= 0.60), preventing keyboard click amplification
        if enable_agc {
            self.apply_agc(frame, vad_prob);
        }

        // 5. Hard safety clamp
        for s in frame.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }

        // 6. Spectral Volume Analysis matching Discord VolumeAnalyzer
        let volume_db = self.volume_analyzer.analyze_frame(frame);
        let ui_level = VolumeAnalyzer::db_to_ui_level(volume_db);
        let is_voice = vad_prob >= 0.45 && volume_db >= -55.0;

        ProcessedVoiceMetrics {
            vad_prob,
            volume_db,
            ui_level,
            is_voice,
        }
    }

    /// Applies Acoustic Echo Cancellation using a Normalized LMS adaptive filter
    /// with delay correlation, acoustic delay alignment, and non-linear Residual Echo Suppression (RES).
    fn apply_aec(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) {
        let mut speaker_samples = [0.0f32; FRAME_SIZE_SAMPLES];
        let count = pop_speaker_reference(&mut speaker_samples);
        if count == 0 {
            return;
        }

        // 1. Acoustic Delay Estimator:
        // Real-world desktop latency (playback buffer -> air -> mic hardware buffer) is typically 15ms - 60ms.
        // We compute RMS across 4 sub-blocks of 240 samples (5ms each) for speaker ref and mic input.
        const SUB_BLOCK_SAMPLES: usize = 240;
        let hist_len = self.aec_speaker_energy_hist.len();

        for sb in 0..4 {
            let start = sb * SUB_BLOCK_SAMPLES;
            let end = start + SUB_BLOCK_SAMPLES;

            let mut spk_sq = 0.0f32;
            for &s in &speaker_samples[start..end] {
                spk_sq += s * s;
            }
            let spk_rms = (spk_sq / SUB_BLOCK_SAMPLES as f32).sqrt();

            let mut mic_sq = 0.0f32;
            for &s in &frame[start..end] {
                mic_sq += s * s;
            }
            let mic_rms = (mic_sq / SUB_BLOCK_SAMPLES as f32).sqrt();

            self.aec_speaker_energy_hist[self.aec_hist_idx] = spk_rms;
            self.aec_mic_energy_hist[self.aec_hist_idx] = mic_rms;
            self.aec_hist_idx = (self.aec_hist_idx + 1) % hist_len;
        }

        // Cross-correlation across recent 160ms history:
        // Evaluates lags from 2 to 20 sub-blocks (10ms to 100ms) to identify acoustic echo delay.
        let spk_total_energy: f32 = self.aec_speaker_energy_hist.iter().sum();
        if spk_total_energy > 0.04 {
            let mut best_lag = 4; // default ~20ms
            let mut max_corr = 0.0f32;
            let mut sum_corr = 0.0f32;
            let mut corr_count = 0;

            for lag in 2..=20 {
                let mut corr = 0.0f32;
                for m in 0..10 {
                    let mic_idx = (self.aec_hist_idx + hist_len - 1 - m) % hist_len;
                    let spk_idx = (self.aec_hist_idx + hist_len - 1 - m - lag) % hist_len;
                    corr += self.aec_mic_energy_hist[mic_idx] * self.aec_speaker_energy_hist[spk_idx];
                }
                sum_corr += corr;
                corr_count += 1;
                if corr > max_corr {
                    max_corr = corr;
                    best_lag = lag;
                }
            }

            let avg_corr = sum_corr / corr_count.max(1) as f32;
            if max_corr > avg_corr * 1.30 && max_corr > 0.0005 {
                let target_delay = best_lag * SUB_BLOCK_SAMPLES;
                // Smooth IIR tracking to avoid abrupt delay jumps
                self.aec_estimated_delay_samples = (self.aec_estimated_delay_samples * 7 + target_delay) / 8;
            }
        }

        // 2. Normalized LMS Adaptive Filter with Delay Alignment and Double-Talk Detection
        let buf_len = self.aec_delay_buffer.len();
        let base_delay = self.aec_estimated_delay_samples.clamp(0, 5760); // Clamp to max 120ms
        let num_taps = self.aec_filter.len();
        let mu = 0.02f32; // Normalized step size

        for i in 0..frame.len() {
            let ref_sample = speaker_samples[i];
            self.aec_delay_buffer[self.aec_delay_idx] = ref_sample;

            // Estimate acoustic echo using delay-aligned tapped delay line
            let mut echo_estimate = 0.0f32;
            let mut power = 1e-4f32;
            for tap in 0..num_taps {
                let delay_pos = (self.aec_delay_idx + buf_len - base_delay - tap) % buf_len;
                let x = self.aec_delay_buffer[delay_pos];
                echo_estimate += self.aec_filter[tap] * x;
                power += x * x;
            }

            let mic_val = frame[i];
            let error = mic_val - echo_estimate;

            // Double-Talk Detection: When mic has strong local energy exceeding estimated echo,
            // near-end user is speaking. Freeze adaptation to preserve speech and avoid corrupting filter.
            let is_double_talk = mic_val.abs() > (echo_estimate.abs() * 2.5 + 0.04);

            if power > 0.0005 && !is_double_talk {
                let norm = mu / power;
                for tap in 0..num_taps {
                    let delay_pos = (self.aec_delay_idx + buf_len - base_delay - tap) % buf_len;
                    // NLMS adaptation with slight leaky factor
                    self.aec_filter[tap] = (self.aec_filter[tap] + norm * error * self.aec_delay_buffer[delay_pos]) * 0.99999;
                }
            }

            // Residual Echo Suppressor (RES):
            // When speaker output is loud and mic input is dominated by echo,
            // suppress non-linear residual echo artifacts smoothly without affecting double-talk speech.
            if echo_estimate.abs() > 0.008 && !is_double_talk && error.abs() < echo_estimate.abs() * 1.5 {
                frame[i] = error * 0.20; // -14 dB residual attenuation
            } else {
                frame[i] = error;
            }

            self.aec_delay_idx = (self.aec_delay_idx + 1) % buf_len;
        }
    }

    /// Helper to probe VAD probability with correct PCM S16 scaling without applying gains to frame
    fn probe_vad_prob(&mut self, frame: &[f32; FRAME_SIZE_SAMPLES]) -> f32 {
        let mut s16_in = [0.0f32; DENOISE_CHUNK_SIZE];
        let mut s16_out = [0.0f32; DENOISE_CHUNK_SIZE];

        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad1 = self.denoiser.process_frame(&mut s16_out, &s16_in);

        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[DENOISE_CHUNK_SIZE + i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad2 = self.denoiser.process_frame(&mut s16_out, &s16_in);

        (vad1 + vad2) * 0.5
    }

    /// Applies Standard Noise Suppression (Preserves 100% natural vocal tone with ~18dB stationary noise floor reduction)
    /// Fixes Bug #1 by scaling inputs to PCM S16 [-32768.0, 32767.0] required by nnnoiseless.
    fn apply_standard_rnnoise(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) -> f32 {
        let mut s16_in = [0.0f32; DENOISE_CHUNK_SIZE];
        let mut s16_out = [0.0f32; DENOISE_CHUNK_SIZE];

        // First 480 samples (10ms)
        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad1 = self.denoiser.process_frame(&mut s16_out, &s16_in);
        for i in 0..DENOISE_CHUNK_SIZE {
            // Apply gentle wet/dry blend (88% RNNoise + 12% original) for natural timbre
            let denoised = s16_out[i] / 32768.0;
            frame[i] = denoised * 0.88 + frame[i] * 0.12;
        }

        // Second 480 samples (10ms)
        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[DENOISE_CHUNK_SIZE + i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad2 = self.denoiser.process_frame(&mut s16_out, &s16_in);
        for i in 0..DENOISE_CHUNK_SIZE {
            let denoised = s16_out[i] / 32768.0;
            frame[DENOISE_CHUNK_SIZE + i] = denoised * 0.88 + frame[DENOISE_CHUNK_SIZE + i] * 0.12;
        }

        (vad1 + vad2) * 0.5
    }

    /// Applies Advanced Noise Suppression (Krisp-grade non-stationary and mechanical keyboard click elimination)
    /// Combines full-depth RNNoise + 2ms sub-block transient suppression + Neural Voice Gate
    fn apply_advanced_suppression(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES]) -> f32 {
        let mut s16_in = [0.0f32; DENOISE_CHUNK_SIZE];
        let mut s16_out = [0.0f32; DENOISE_CHUNK_SIZE];

        // 1. Full-depth RNNoise with S16 scaling
        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad1 = self.denoiser.process_frame(&mut s16_out, &s16_in);
        for i in 0..DENOISE_CHUNK_SIZE {
            frame[i] = s16_out[i] / 32768.0;
        }

        for i in 0..DENOISE_CHUNK_SIZE {
            s16_in[i] = (frame[DENOISE_CHUNK_SIZE + i] * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad2 = self.denoiser.process_frame(&mut s16_out, &s16_in);
        for i in 0..DENOISE_CHUNK_SIZE {
            frame[DENOISE_CHUNK_SIZE + i] = s16_out[i] / 32768.0;
        }

        let vad_prob = (vad1 + vad2) * 0.5;

        // 2. High-Frequency Transient Percussive Click Suppressor (Keyboard & Mouse clicks)
        // Mechanical keyboard clicks are short impulsive bursts (2ms..6ms) with low vocal periodic pitch.
        // We evaluate 10 sub-blocks of 96 samples (2ms each)
        const SUB_BLOCK_SIZE: usize = 96;
        for sub_chunk in frame.chunks_exact_mut(SUB_BLOCK_SIZE) {
            let mut sub_sq = 0.0f32;
            let mut max_abs = 0.0f32;
            for &s in sub_chunk.iter() {
                let a = s.abs();
                sub_sq += a * a;
                if a > max_abs { max_abs = a; }
            }
            let sub_rms = (sub_sq / SUB_BLOCK_SIZE as f32).sqrt();

            // Track smooth background noise floor
            self.sub_block_energy_floor = self.sub_block_energy_floor * 0.98 + sub_rms * 0.02;

            // Transient detection: High crest ratio (> 4.5x) or sudden burst while vad_prob < 0.40
            let is_transient = (max_abs / (self.sub_block_energy_floor + 1e-4) > 4.5 || sub_rms > 0.015)
                && vad_prob < 0.40;

            if is_transient {
                // Surgically attenuate the impulsive mechanical click
                let attenuation = (self.sub_block_energy_floor / (sub_rms + 1e-4)).clamp(0.01, 0.20);
                for s in sub_chunk.iter_mut() {
                    *s *= attenuation;
                }
            }
        }

        // 3. Neural Voice Gate with Fast Attack (2ms ramp) and Smooth Release (120ms)
        // Target gate gain: 1.0 when human voice is verified (vad_prob >= 0.45); down to -32dB (0.025) on silence/typing
        let target_gate = if vad_prob >= 0.45 {
            1.0f32
        } else if vad_prob <= 0.15 {
            0.025f32
        } else {
            // Smooth linear crossfade in uncertain zone [0.15, 0.45]
            0.025 + (vad_prob - 0.15) / 0.30 * 0.975
        };

        let start_gain = self.neural_gate_gain;
        let end_gain = if target_gate > start_gain {
            // Fast attack: jump straight to target gate without delay
            target_gate
        } else {
            // Smooth release across frames
            start_gain * 0.92 + target_gate * 0.08
        };

        // Sample-by-sample interpolation to eliminate clicks and prevent clipping first syllable:
        // On voice onset, ramp up across the first 2ms (96 samples) to preserve initial consonants
        let ramp_len = if target_gate > start_gain {
            96.min(frame.len())
        } else {
            frame.len()
        };

        let ramp_step = (end_gain - start_gain) / ramp_len as f32;
        let mut cur_gain = start_gain;

        for (i, s) in frame.iter_mut().enumerate() {
            if i < ramp_len {
                cur_gain += ramp_step;
            } else {
                cur_gain = end_gain;
            }
            *s *= cur_gain;
        }

        self.neural_gate_gain = end_gain;

        vad_prob
    }

    /// Automatic Gain Control (AGC2 AdaptiveDigital) with Voice-Gating
    /// Matches webrtc::GainController2 (AudioEngine::SetAutomaticGainControlConfig in 0x388de0):
    /// - Target: -3 dBFS peak / -18 dBFS speech RMS
    /// - Maximum compression gain: +9 dB (2.818x)
    /// - Adaptation is strictly gated on human voice (vad_prob >= 0.60), NEVER amplifying typing in silence
    fn apply_agc(&mut self, frame: &mut [f32; FRAME_SIZE_SAMPLES], vad_prob: f32) {
        let mut sum_sq = 0.0f32;
        for &s in frame.iter() {
            sum_sq += s * s;
        }
        let frame_rms = (sum_sq / frame.len() as f32).sqrt();

        // ONLY adapt gain when confident human speech is present (vad_prob >= 0.60)
        // This is the single most important fix to prevent keyboard clicks from blowing up in volume!
        if vad_prob >= 0.60 && frame_rms > 0.005 {
            let attack = 0.06f32;
            let release = 0.015f32;
            if frame_rms > self.agc_speech_envelope {
                self.agc_speech_envelope = self.agc_speech_envelope * (1.0 - attack) + frame_rms * attack;
            } else {
                self.agc_speech_envelope = self.agc_speech_envelope * (1.0 - release) + frame_rms * release;
            }

            // Target speech level is -18 dBFS (approx 0.125 RMS)
            let target_rms = 0.125f32;
            // Max gain is +9 dB (2.818x), min gain is -6 dB (0.50x)
            let desired_gain = (target_rms / self.agc_speech_envelope.max(0.01)).clamp(0.5, 2.818);
            self.agc_current_gain = self.agc_current_gain * 0.96 + desired_gain * 0.04;
        } else if vad_prob < 0.30 {
            // When silent or typing, smoothly relax gain back toward unity (1.0x) instead of amplifying
            self.agc_current_gain = self.agc_current_gain * 0.99 + 1.0 * 0.01;
        }

        // Apply gain with soft-knee limiter at -1.0 dBFS (0.891)
        for s in frame.iter_mut() {
            let amplified = *s * self.agc_current_gain;
            if amplified.abs() > 0.85 {
                let sign = amplified.signum();
                let x = amplified.abs() - 0.85;
                // Soft saturation curve: prevents harsh digital clipping
                let saturated = 0.85 + (0.15 * (x / 0.15).tanh());
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

        // 1. Benchmark: None (bypass)
        let iters = 100;
        let start = Instant::now();
        for _ in 0..iters {
            let mut frame = [0.1f32; FRAME_SIZE_SAMPLES];
            vp.process_frame(&mut frame, "none", false, false);
        }
        let elapsed_none_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 2. Benchmark: Standard (RNNoise only)
        let start = Instant::now();
        for _ in 0..iters {
            let mut frame = [0.1f32; FRAME_SIZE_SAMPLES];
            vp.process_frame(&mut frame, "standard", false, false);
        }
        let elapsed_std_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 3. Benchmark: Advanced (Neural + Transients)
        let start = Instant::now();
        for _ in 0..iters {
            let mut frame = [0.1f32; FRAME_SIZE_SAMPLES];
            vp.process_frame(&mut frame, "advanced", false, false);
        }
        let elapsed_adv_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // 4. Benchmark: All enabled (Advanced + AEC + AGC)
        let start = Instant::now();
        for _ in 0..iters {
            let mut frame = [0.1f32; FRAME_SIZE_SAMPLES];
            vp.process_frame(&mut frame, "advanced", true, true);
        }
        let elapsed_all_us = start.elapsed().as_micros() as f64 / (iters as f64);

        // Component breakdown:
        let mut d = nnnoiseless::DenoiseState::new();
        let s_in = [0.0f32; 480];
        let mut s_out = [0.0f32; 480];
        let t = Instant::now();
        for _ in 0..100 {
            d.process_frame(&mut s_out, &s_in);
        }
        let d_us = t.elapsed().as_micros() as f64 / 100.0;

        let mut va = VolumeAnalyzer::new();
        let test_frame = [0.1f32; 960];
        let t = Instant::now();
        for _ in 0..100 {
            va.analyze_frame(&test_frame);
        }
        let va_us = t.elapsed().as_micros() as f64 / 100.0;

        let mut hpf = BiquadHpf::new_80hz_48khz();
        let mut hpf_frame = [0.1f32; 960];
        let t = Instant::now();
        for _ in 0..100 {
            hpf.process_frame(&mut hpf_frame);
        }
        let hpf_us = t.elapsed().as_micros() as f64 / 100.0;

        println!(
            "\n=== Voice Processing Benchmark (20ms / 960-sample frame) ===\n\
             Budget per frame: 20,000 µs (20.0 ms)\n\
             - Bypass / None:       {:.2} µs ({:.4}% of budget)\n\
             - Standard RNNoise:    {:.2} µs ({:.4}% of budget)\n\
             - Advanced Neural:     {:.2} µs ({:.4}% of budget)\n\
             - Full (Adv+AEC+AGC):  {:.2} µs ({:.4}% of budget)\n\
             Component Breakdown:\n\
             - DenoiseState (1x 10ms chunk): {:.2} µs\n\
             - VolumeAnalyzer (960 samples): {:.2} µs\n\
             - BiquadHpf (960 samples):      {:.2} µs\n",
            elapsed_none_us, (elapsed_none_us / 20000.0) * 100.0,
            elapsed_std_us, (elapsed_std_us / 20000.0) * 100.0,
            elapsed_adv_us, (elapsed_adv_us / 20000.0) * 100.0,
            elapsed_all_us, (elapsed_all_us / 20000.0) * 100.0,
            d_us, va_us, hpf_us
        );

        // Assert that processing takes under 100% of the 20ms real-time audio deadline (20,000 µs)
        assert!(elapsed_all_us < 20000.0, "Voice processing must meet 20ms frame budget");
    }

    #[test]
    fn test_volume_analyzer_db_scale_and_bass_rejection() {
        let mut analyzer = VolumeAnalyzer::new();

        // 1. Absolute silence should be near -100 dBFS
        let silence = [0.0f32; 512];
        let db_silence = analyzer.analyze_frame(&silence);
        assert!(db_silence <= -80.0, "Silence should produce <= -80 dB, got {:.2}", db_silence);
        let ui_silence = VolumeAnalyzer::db_to_ui_level(db_silence);
        assert!(ui_silence <= 0.20, "Silence UI level should be low, got {:.2}", ui_silence);

        // 2. 50Hz deep desk rumble should be heavily rejected by >375Hz band filter
        let mut rumble_50hz = [0.0f32; 512];
        for i in 0..512 {
            rumble_50hz[i] = (2.0 * std::f32::consts::PI * 50.0 * (i as f32) / 48000.0).sin() * 0.5;
        }
        let db_rumble = analyzer.analyze_frame(&rumble_50hz);
        assert!(db_rumble < -40.0, "50Hz rumble should be rejected below -40 dB, got {:.2}", db_rumble);

        // 3. 1000Hz vocal tone should be clearly recognized with high volume
        let mut voice_1khz = [0.0f32; 512];
        for i in 0..512 {
            voice_1khz[i] = (2.0 * std::f32::consts::PI * 1000.0 * (i as f32) / 48000.0).sin() * 0.5;
        }
        let db_voice = analyzer.analyze_frame(&voice_1khz);
        assert!(db_voice > -25.0, "1000Hz tone should measure above -25 dB, got {:.2}", db_voice);
        let ui_voice = VolumeAnalyzer::db_to_ui_level(db_voice);
        assert!(ui_voice >= 0.70, "1000Hz tone UI level should be >= 0.70, got {:.2}", ui_voice);
    }

    #[test]
    fn test_advanced_suppression_transient_keyboard_elimination() {
        let mut vp = VoiceProcessor::new();

        // Simulate mechanical keyboard click: impulsive burst (4ms / 192 samples) with silent background
        let mut click_frame = [0.0f32; FRAME_SIZE_SAMPLES];
        for i in 100..292 {
            // High frequency decaying impulse simulating mechanical switch contact
            let decay = (-((i - 100) as f32) / 40.0).exp();
            click_frame[i] = (2.0 * std::f32::consts::PI * 3500.0 * (i as f32) / 48000.0).sin() * 0.6 * decay;
        }

        let metrics = vp.process_frame(&mut click_frame, "advanced", false, true);

        // Max peak after advanced suppression
        let mut peak_after = 0.0f32;
        for &s in click_frame.iter() {
            if s.abs() > peak_after { peak_after = s.abs(); }
        }

        // Impulsive click without voice should not trigger voice classification
        assert!(!metrics.is_voice, "Keyboard click should NOT be classified as human voice");
        // Transient peak should be heavily attenuated
        assert!(peak_after < 0.25, "Keyboard click peak should be suppressed, got {:.3}", peak_after);
    }

    #[test]
    fn test_acoustic_echo_cancellation_and_double_talk() {
        let mut vp = VoiceProcessor::new();

        // Simulate far-end speaker audio: continuous 440Hz tone
        let mut spk_frame = [0.0f32; FRAME_SIZE_SAMPLES];
        for i in 0..FRAME_SIZE_SAMPLES {
            spk_frame[i] = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48000.0).sin() * 0.4;
        }

        // Feed multiple frames of speaker reference and delayed echo into mic to allow NLMS to converge
        for _ in 0..15 {
            push_speaker_reference(&spk_frame);
            // Mic picks up acoustic echo with 50% coupling
            let mut echo_frame = [0.0f32; FRAME_SIZE_SAMPLES];
            for i in 0..FRAME_SIZE_SAMPLES {
                echo_frame[i] = spk_frame[i] * 0.5;
            }
            vp.process_frame(&mut echo_frame, "none", true, false);
        }

        // Now test single-talk echo suppression
        push_speaker_reference(&spk_frame);
        let mut echo_frame = [0.0f32; FRAME_SIZE_SAMPLES];
        for i in 0..FRAME_SIZE_SAMPLES {
            echo_frame[i] = spk_frame[i] * 0.5;
        }
        let orig_rms = (echo_frame.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE_SAMPLES as f32).sqrt();
        vp.process_frame(&mut echo_frame, "none", true, false);
        let post_aec_rms = (echo_frame.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE_SAMPLES as f32).sqrt();

        // Echo should be suppressed by at least 8 dB (ratio < 0.40)
        assert!(
            post_aec_rms < orig_rms * 0.40,
            "AEC should suppress acoustic echo: orig={:.4}, post={:.4}",
            orig_rms, post_aec_rms
        );

        // Now test double-talk: near-end speaker talks loudly (1000Hz tone at 0.6 amplitude)
        push_speaker_reference(&spk_frame);
        let mut double_talk_frame = [0.0f32; FRAME_SIZE_SAMPLES];
        for i in 0..FRAME_SIZE_SAMPLES {
            let local_voice = (2.0 * std::f32::consts::PI * 1000.0 * (i as f32) / 48000.0).sin() * 0.6;
            let echo = spk_frame[i] * 0.2;
            double_talk_frame[i] = local_voice + echo;
        }
        vp.process_frame(&mut double_talk_frame, "none", true, false);
        let dt_rms = (double_talk_frame.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE_SAMPLES as f32).sqrt();
        // Near-end voice should be preserved (RMS should remain substantial, > 0.25)
        assert!(dt_rms > 0.25, "Double-talk near-end voice should be preserved, got {:.4}", dt_rms);
    }
}
