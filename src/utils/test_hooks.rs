//! Test hooks for running two instances on one machine and measuring voice end to end.
//! Compiled only with `--features automation`, and each is inert unless its
//! environment variable is set.
//!
//! ```text
//! LITECORD_TEST_ACCOUNT=<user id or name>   log in with that saved account; never write the vault
//! LITECORD_TEST_MIC=loopback               capture the default output device instead of a microphone
//! LITECORD_TEST_TIMER_PACING=1            send voice on the old 20 ms timer with 1 ms resolution (A/B baseline)
//! LITECORD_TEST_MUTE_OUTPUT=1              play nothing, UI sounds included (keeps a loopback "mic" clean)
//! LITECORD_TEST_VOICE_LOG=<file.csv>       one row per mic block, sent and received voice packet
//! LITECORD_TEST_RX_PCM=<file.f32>          decoded received voice, 48 kHz mono f32, in arrival order
//! LITECORD_TEST_TX_PCM=<file.f32>          what the microphone captured, 48 kHz mono f32
//! LITECORD_TEST_DECODE_DELAY_MS=<ms>       slow every video decode down (simulates a busy receiver)
//! LITECORD_TEST_DECODE_STALL=<ms>,<every ms> stall the decoder now and then (simulates game hitches)
//! LITECORD_TEST_HIDE_OUTPUT_SECS=<s>       voice sees no output device for the first <s> seconds
//! LITECORD_TEST_MIC=synth                  a generated voice-like tone instead of a microphone
//! LITECORD_TEST_SILENT_PLAYBACK=1          mix received voice as usual (and log it), then play silence
//! ```

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Convert received frames at full size even when shown smaller (A/B of the scaled
/// conversion; set through the automation port).
pub static FULL_SIZE_CONVERSION: AtomicBool = AtomicBool::new(false);

/// Decode received streams with OpenH264 even when the GPU could (A/B; automation port).
pub static CPU_DECODE: AtomicBool = AtomicBool::new(false);

/// Drop the gateway connection on its next message, like a network loss (automation port).
pub static DROP_GATEWAY: AtomicBool = AtomicBool::new(false);

/// Stop the voice output stream as a failing audio device would (automation port).
pub static BREAK_VOICE_OUTPUT: AtomicBool = AtomicBool::new(false);

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn test_account() -> Option<String> {
    env("LITECORD_TEST_ACCOUNT")
}

pub fn mic_from_loopback() -> bool {
    env("LITECORD_TEST_MIC").as_deref() == Some("loopback")
}

pub fn timer_pacing() -> bool {
    env("LITECORD_TEST_TIMER_PACING").is_some()
}

pub fn decode_delay() -> Option<std::time::Duration> {
    static DELAY: OnceLock<Option<std::time::Duration>> = OnceLock::new();
    *DELAY.get_or_init(|| env("LITECORD_TEST_DECODE_DELAY_MS")?.parse().ok().map(std::time::Duration::from_millis))
}

/// A stall to apply now, if one is due (LITECORD_TEST_DECODE_STALL=<ms>,<every ms>).
pub fn decode_stall_due() -> Option<std::time::Duration> {
    static CFG: OnceLock<Option<(u64, u64)>> = OnceLock::new();
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let (stall_ms, every_ms) = (*CFG.get_or_init(|| {
        let v = env("LITECORD_TEST_DECODE_STALL")?;
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    }))?;
    let mut last = LAST.lock().ok()?;
    let now = Instant::now();
    match *last {
        Some(t) if now.duration_since(t).as_millis() < every_ms as u128 => None,
        _ => {
            *last = Some(now);
            Some(std::time::Duration::from_millis(stall_ms))
        }
    }
}

/// True while the voice output device should look absent (LITECORD_TEST_HIDE_OUTPUT_SECS).
pub fn hide_output_device() -> bool {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    env("LITECORD_TEST_HIDE_OUTPUT_SECS")
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|secs| start.elapsed().as_secs() < secs)
}

pub fn mic_synth() -> bool {
    env("LITECORD_TEST_MIC").as_deref() == Some("synth")
}

pub fn silent_playback() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env("LITECORD_TEST_SILENT_PLAYBACK").is_some())
}

/// Feeds the voice pipeline a generated tone, paced like a microphone (10 ms blocks),
/// until `stop` is set. 300 ms of tone then 200 ms of silence, so the voice activity
/// detection opens and closes as it does for speech.
pub fn run_synth_mic(stop: std::sync::Arc<AtomicBool>) {
    use std::sync::atomic::Ordering;
    let _fine = crate::utils::timer_resolution::FineTimerGuard::new();
    let queue = crate::gateway::get_mic_pcm_queue();
    let start = Instant::now();
    let mut produced: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let due = (start.elapsed().as_micros() as u64 * 48) / 1000;
        if due >= produced + 480 {
            let block: Vec<f32> = (0..480u64)
                .map(|i| {
                    let n = produced + i;
                    let on = (n / 4800) % 5 < 3;
                    if on { 0.3 * (n as f32 * 2.0 * std::f32::consts::PI * 330.0 / 48000.0).sin() } else { 0.0 }
                })
                .collect();
            produced += 480;
            let level = if block.iter().any(|s| *s != 0.0) { 1.0 } else { 0.0 };
            crate::gateway::set_self_mic_level(level);
            if let Ok(mut q) = queue.lock() {
                crate::audio::engine::push_to_mic_queues(&mut q, &block, &block, level);
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// Called at the end of the voice output callback: logs whether the block carries
/// sound (`play`, value = audible samples) and silences it under SILENT_PLAYBACK.
pub fn voice_played_f32(out: &mut [f32]) {
    let n = out.iter().filter(|s| s.abs() > 1e-4).count();
    if n > 0 {
        record("play", n as u32, false);
    }
    if silent_playback() {
        out.fill(0.0);
    }
}

pub fn voice_played_i16(out: &mut [i16]) {
    let n = out.iter().filter(|s| s.unsigned_abs() > 3).count();
    if n > 0 {
        record("play", n as u32, false);
    }
    if silent_playback() {
        out.fill(0);
    }
}

/// A received voice packet that DAVE could not decrypt.
pub fn dave_decrypt_failed() {
    record("dave_fail", 0, false);
}

/// The voice output stream was (re)started.
pub fn voice_output_started() {
    record("out_start", 0, false);
}

pub fn mute_output() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env("LITECORD_TEST_MUTE_OUTPUT").is_some())
}

struct VoiceLog {
    file: std::io::BufWriter<std::fs::File>,
    t0: Instant,
}

fn voice_log() -> Option<&'static Mutex<VoiceLog>> {
    static LOG: OnceLock<Option<Mutex<VoiceLog>>> = OnceLock::new();
    LOG.get_or_init(|| {
        let path = env("LITECORD_TEST_VOICE_LOG")?;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
        let _ = writeln!(file, "t_us,event,value,flag");
        // Wall clock of t_us = 0, so logs of two instances line up with each other.
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let _ = writeln!(file, "0,t0_unix_s,{},0", unix.as_secs());
        let _ = writeln!(file, "0,t0_unix_ms,{},0", unix.subsec_millis());
        Some(Mutex::new(VoiceLog { file, t0: Instant::now() }))
    })
    .as_ref()
}

/// One row per event: `mic` (samples captured), `tx` (packet sent; value = 1 for speech,
/// flag = 1 when the frame wait timed out) or `rx` (packet received; value = RTP sequence).
fn record(event: &str, value: u32, flag: bool) {
    let Some(log) = voice_log() else { return };
    let Ok(mut log) = log.lock() else { return };
    let t = log.t0.elapsed().as_micros();
    let _ = writeln!(log.file, "{t},{event},{value},{}", flag as u8);
    let _ = log.file.flush();
}

/// Samples the microphone callback delivered (at 48 kHz).
pub fn mic_captured(samples: usize) {
    record("mic", samples as u32, false);
}

/// One RTP voice packet sent. `from_fallback`: the frame wait timed out (no mic data).
pub fn voice_sent(speech: bool, from_fallback: bool) {
    record("tx", speech as u32, from_fallback);
}

/// One RTP voice packet received from another user.
pub fn voice_received(seq: u16) {
    record("rx", seq as u32, false);
}

/// Captured microphone samples (48 kHz mono), appended as f32.
pub fn voice_captured(samples: &[f32]) {
    static PCM: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    let Some(out) = PCM
        .get_or_init(|| {
            let path = env("LITECORD_TEST_TX_PCM")?;
            std::fs::File::create(path).ok().map(|f| Mutex::new(std::io::BufWriter::new(f)))
        })
        .as_ref()
    else {
        return;
    };
    if let Ok(mut out) = out.lock() {
        for &s in samples {
            let _ = out.write_all(&s.to_le_bytes());
        }
        let _ = out.flush();
    }
}

/// Decoded received voice (stereo pairs at 48 kHz), appended as mono f32.
pub fn voice_decoded(pairs: &[(f32, f32)]) {
    record("dec", pairs.len() as u32, false);
    static PCM: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    let Some(out) = PCM
        .get_or_init(|| {
            let path = env("LITECORD_TEST_RX_PCM")?;
            std::fs::File::create(path).ok().map(|f| Mutex::new(std::io::BufWriter::new(f)))
        })
        .as_ref()
    else {
        return;
    };
    if let Ok(mut out) = out.lock() {
        for &(l, r) in pairs {
            let _ = out.write_all(&((l + r) * 0.5).to_le_bytes());
        }
        let _ = out.flush();
    }
}
