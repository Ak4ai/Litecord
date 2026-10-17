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
//! ```

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

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

/// Decoded received voice (stereo pairs at 48 kHz), appended as mono f32.
pub fn voice_decoded(pairs: &[(f32, f32)]) {
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
