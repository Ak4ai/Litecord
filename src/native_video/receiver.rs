//! Receive pipeline for one native video connection.
//!
//! A connection is either the parent voice socket (camera) or a Go Live stream
//! socket.  The owner of the frames is resolved per SSRC, except on stream
//! connections where every frame belongs to the stream's owner.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use davey::{DaveSession, MediaType};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use tokio::net::UdpSocket;

use super::crypto::{decrypt_payload, protect_rtcp, TransportMode};
use super::h264::H264Depacketizer;
use super::jitter::JitterBuffer;
use super::{emit_frame, rtcp, rtp, user_for_video_ssrc};
use crate::gateway::get_user_name;
use crate::screen_capture::types::{
    get_active_stream_frames, mark_stream_started, set_peer_streaming,
};
use crate::screen_capture::video_pipeline::{decode_video_frame, Decoded, StreamDecoders};
use crate::screen_capture::watch::set_wanted;

/// How often a keyframe may be requested for the same SSRC.
const PLI_INTERVAL: Duration = Duration::from_millis(700);
/// How often a NACK may be sent for the same SSRC.
const NACK_INTERVAL: Duration = Duration::from_millis(40);
/// A stream is marked as stopped when no frame arrived for this long.
const STREAM_IDLE: Duration = Duration::from_secs(8);

pub struct NativeVideoContext {
    /// Transport key from the voice opcode 4 Session Description.
    pub secret_key: Arc<Mutex<Option<Vec<u8>>>>,
    /// Selected transport mode (`aead_aes256_gcm_rtpsize` / `..._xchacha20...`).
    pub mode: Arc<Mutex<String>>,
    /// DAVE session of this voice connection (parent or stream specific).
    pub dave: Arc<Mutex<Option<DaveSession>>>,
    /// Audio/video SSRC to user map filled from voice opcode 12.
    pub ssrc_to_uid: Arc<Mutex<HashMap<u32, u64>>>,
    /// On a Go Live connection every frame belongs to this user.
    pub stream_owner: Option<u64>,
    /// Socket used for RTCP feedback (PLI/NACK).
    pub socket: Option<Arc<UdpSocket>>,
    pub target: Option<SocketAddr>,
    /// Local SSRC used as RTCP sender SSRC.
    pub sender_ssrc: u32,
    /// Nonce counter for outgoing RTCP.  On the parent connection this is the
    /// same atomic as the audio sender, so one sequence covers every packet
    /// encrypted with that key.
    pub rtcp_counter: Arc<AtomicU32>,
}

pub struct NativeVideoSession {
    jitter: HashMap<u32, JitterBuffer>,
    depacketizers: HashMap<u32, H264Depacketizer>,
    decoders: StreamDecoders,
    started: HashSet<u64>,
    fps: HashMap<u64, FpsCounter>,
    last_frame: HashMap<u64, Instant>,
    last_pli: HashMap<u32, Instant>,
    last_nack: HashMap<u32, Instant>,
}

struct FpsCounter {
    frames: u32,
    since: Instant,
    value: f32,
}

impl Default for NativeVideoSession {
    fn default() -> Self {
        Self {
            jitter: HashMap::new(),
            depacketizers: HashMap::new(),
            decoders: StreamDecoders::default(),
            started: HashSet::new(),
            fps: HashMap::new(),
            last_frame: HashMap::new(),
            last_pli: HashMap::new(),
            last_nack: HashMap::new(),
        }
    }
}

impl NativeVideoSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one UDP datagram.  Audio packets and RTCP are ignored here; the
    /// caller keeps handling Opus exactly as before.
    pub fn ingest(&mut self, ctx: &NativeVideoContext, datagram: &[u8]) {
        if datagram.len() < 12 || rtp::is_rtcp(datagram) {
            return;
        }
        let Some(header) = rtp::parse(datagram) else {
            return;
        };
        // Opus is payload type 120 in Discord.  Anything else on this socket is
        // video (or a probe).
        if header.payload_type == 120 || header.payload_type == 0 {
            return;
        }

        let uid = ctx
            .stream_owner
            .or_else(|| ctx.ssrc_to_uid.lock().ok().and_then(|map| map.get(&header.ssrc).copied()))
            .or_else(|| user_for_video_ssrc(header.ssrc));
        let Some(uid) = uid else {
            return;
        };

        let key = ctx
            .secret_key
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(key) = key else {
            return;
        };
        let mode = ctx
            .mode
            .lock()
            .map(|guard| TransportMode::from_name(&guard))
            .unwrap_or(TransportMode::Aes256Gcm);

        let Some(mut payload) = decrypt_payload(&key, mode, datagram, &header) else {
            return;
        };

        // RTX retransmissions use their own SSRC and prefix the payload with the
        // original sequence number; feed them to the primary stream.
        let info = crate::native_video::video_info(header.ssrc);
        let is_rtx = info
            .as_ref()
            .is_some_and(|i| i.rtx_ssrc == header.ssrc && i.ssrc != header.ssrc);
        let media_ssrc = if is_rtx {
            info.as_ref().map(|i| i.ssrc).filter(|s| *s != 0).unwrap_or(header.ssrc)
        } else {
            header.ssrc
        };
        let sequence = if is_rtx {
            if payload.len() < 2 {
                return;
            }
            let original = u16::from_be_bytes([payload[0], payload[1]]);
            payload.drain(..2);
            original
        } else {
            header.sequence
        };

        let now = Instant::now();
        let (ordered, missing) = self
            .jitter
            .entry(media_ssrc)
            .or_default()
            .push(sequence, payload, header.marker);

        if !missing.is_empty() {
            self.send_nack(ctx, media_ssrc, &missing);
        }

        // A NACKed packet may never arrive; continue past it instead of freezing.
        let stale = self
            .jitter
            .get_mut(&media_ssrc)
            .map(|jitter| jitter.flush_stale(Duration::from_millis(80), now))
            .unwrap_or_default();

        for (_seq, payload, marker) in ordered.into_iter().chain(stale) {
            let complete = self
                .depacketizers
                .entry(media_ssrc)
                .or_default()
                .push(&payload, marker);
            if let Some(frame) = complete {
                self.handle_frame(ctx, uid, media_ssrc, &frame);
            }
        }
    }

    fn handle_frame(&mut self, ctx: &NativeVideoContext, uid: u64, ssrc: u32, frame: &[u8]) {
        // Native frames must show up without the LTPV "Watch" button: mark the
        // stream wanted *before* announcing it, otherwise the UI state callback
        // hides it.  Re-asserting both also covers the parent/stream sessions
        // clearing each other's state.
        set_wanted(uid, true);
        set_peer_streaming(uid, true);
        let Some(frame) = decrypt_dave(ctx, uid, frame) else {
            self.send_pli(ctx, ssrc);
            return;
        };

        let decoded = decode_video_frame(&mut self.decoders, uid, &frame);
        let now = Instant::now();
        let fps = self
            .fps
            .entry(uid)
            .or_insert_with(|| FpsCounter { frames: 0, since: now, value: 0.0 });
        fps.frames += 1;
        if fps.since.elapsed() >= Duration::from_secs(1) {
            fps.value = fps.frames as f32 / fps.since.elapsed().as_secs_f32().max(0.001);
            fps.frames = 0;
            fps.since = now;
        }
        let fps_value = fps.value;
        self.last_frame.insert(uid, now);

        match decoded {
            Decoded::Picture(buffer, _width, height) => {
                self.mark_started(uid);
                if let Ok(mut frames) = get_active_stream_frames().lock() {
                    frames.insert(uid, buffer.clone());
                }
                let label = format!("{}p {:.1}fps (H.264)", height, fps_value.max(1.0));
                emit_frame(uid, get_user_name(uid), label, buffer);
            }
            Decoded::ShownByGpu(height) => {
                self.mark_started(uid);
                let label = format!("{}p {:.1}fps (H.264)", height, fps_value.max(1.0));
                emit_frame(
                    uid,
                    get_user_name(uid),
                    label,
                    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1),
                );
            }
            Decoded::NeedKeyframe | Decoded::Failed => {
                self.send_pli(ctx, ssrc);
            }
            Decoded::Nothing => {}
        }
    }

    fn mark_started(&mut self, uid: u64) {
        if self.started.insert(uid) {
            // Native camera/Go Live should appear without the LTPV "Watch"
            // button: mark the stream as wanted before announcing it.
            set_wanted(uid, true);
            mark_stream_started(uid);
            set_peer_streaming(uid, true);
            log::info!("🎥 [NATIVE VIDEO] Primeiro quadro nativo do usuário {} recebido", uid);
        }
    }

    /// Drops streams that stopped sending frames (Go Live ended, camera off).
    pub fn tick(&mut self, ctx: &NativeVideoContext) {
        let now = Instant::now();
        let stale: Vec<u64> = self
            .last_frame
            .iter()
            .filter(|(_, at)| now.duration_since(**at) > STREAM_IDLE)
            .map(|(uid, _)| *uid)
            .collect();
        for uid in stale {
            self.last_frame.remove(&uid);
            self.started.remove(&uid);
            // On the parent (camera) connection the same user may still be
            // sending through a Go Live connection; STREAM_DELETE (or that
            // viewer's own tick) clears the state then.
            if ctx.stream_owner.is_some() || !super::stream::has_viewer_for(uid) {
                set_peer_streaming(uid, false);
            }
        }
    }

    fn send_pli(&mut self, ctx: &NativeVideoContext, ssrc: u32) {
        let now = Instant::now();
        if self
            .last_pli
            .get(&ssrc)
            .is_some_and(|last| now.duration_since(*last) < PLI_INTERVAL)
        {
            return;
        }
        self.last_pli.insert(ssrc, now);
        self.send_rtcp(ctx, rtcp::build_pli(ctx.sender_ssrc, ssrc));
    }

    /// Sends one RTCP packet, transport-encrypted with the connection key when
    /// it is available (the SFU ignores plaintext feedback).
    fn send_rtcp(&self, ctx: &NativeVideoContext, packet: Vec<u8>) {
        let (Some(socket), Some(target)) = (&ctx.socket, ctx.target) else {
            return;
        };
        let key = ctx
            .secret_key
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let wire = match key {
            Some(key) => {
                let mode = ctx
                    .mode
                    .lock()
                    .map(|guard| TransportMode::from_name(&guard))
                    .unwrap_or(TransportMode::Aes256Gcm);
                let counter = ctx.rtcp_counter.fetch_add(1, Ordering::Relaxed);
                protect_rtcp(&key, mode, &packet, counter).unwrap_or(packet)
            }
            None => packet,
        };
        let _ = socket.try_send_to(&wire, target);
    }

    fn send_nack(&mut self, ctx: &NativeVideoContext, ssrc: u32, missing: &[u16]) {
        let now = Instant::now();
        if self
            .last_nack
            .get(&ssrc)
            .is_some_and(|last| now.duration_since(*last) < NACK_INTERVAL)
        {
            return;
        }
        self.last_nack.insert(ssrc, now);
        if let Some(packet) = rtcp::build_nack(ctx.sender_ssrc, ssrc, missing) {
            self.send_rtcp(ctx, packet);
        }
    }

    /// Resets the decoders (e.g. after a DAVE transition).
    pub fn reset(&mut self) {
        self.jitter.clear();
        self.depacketizers.clear();
        self.decoders = StreamDecoders::default();
        self.started.clear();
        self.fps.clear();
        self.last_frame.clear();
    }
}

/// The two bytes DAVE appends to every encrypted frame.  Their presence means
/// the payload is ciphertext: showing it as plaintext would feed garbage to the
/// decoder (and could briefly paint noise on screen).
const DAVE_MARKER: [u8; 2] = [0xFA, 0xFA];

fn has_dave_marker(frame: &[u8]) -> bool {
    frame.ends_with(&DAVE_MARKER)
}

fn decrypt_dave(ctx: &NativeVideoContext, uid: u64, frame: &[u8]) -> Option<Vec<u8>> {
    let mut guard = ctx.dave.lock().unwrap_or_else(|e| e.into_inner());
    let Some(session) = guard.as_mut() else {
        // No DAVE group at all: media is plain text.
        return Some(frame.to_vec());
    };
    if !session.is_ready() {
        // The group is still forming.  Frames marked as encrypted cannot be
        // shown yet; unencrypted ones (the usual transition window) can.
        if has_dave_marker(frame) {
            return None;
        }
        return Some(frame.to_vec());
    }

    match session.decrypt(uid, MediaType::VIDEO, frame) {
        Ok(clear) => Some(clear),
        Err(error) => {
            // During the transition to plaintext, frames are not encrypted.
            // The DAVE magic tail marks a frame that must not be displayed in
            // clear text.
            if has_dave_marker(frame) {
                log::trace!("[DAVE VIDEO] Falha ao decifrar vídeo do usuário {}: {:?}", uid, error);
                None
            } else {
                Some(frame.to_vec())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    fn context(dave: Option<DaveSession>) -> NativeVideoContext {
        NativeVideoContext {
            secret_key: Arc::new(Mutex::new(None)),
            mode: Arc::new(Mutex::new("aead_aes256_gcm_rtpsize".to_string())),
            dave: Arc::new(Mutex::new(dave)),
            ssrc_to_uid: Arc::new(Mutex::new(HashMap::new())),
            stream_owner: None,
            socket: None,
            target: None,
            sender_ssrc: 0,
            rtcp_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    fn frame(tail: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, 0x65, 0x88];
        out.extend_from_slice(tail);
        out
    }

    #[test]
    fn without_dave_every_frame_is_plaintext() {
        let ctx = context(None);
        assert_eq!(decrypt_dave(&ctx, 1, &frame(&[])), Some(frame(&[])));
        assert_eq!(decrypt_dave(&ctx, 1, &frame(&DAVE_MARKER)), Some(frame(&DAVE_MARKER)));
    }

    #[test]
    fn frames_with_the_dave_tail_are_dropped_until_the_group_is_ready() {
        let session = DaveSession::new(NonZeroU16::MIN, 1, 1, None).expect("DAVE session");
        assert!(!session.is_ready());
        let ctx = context(Some(session));

        assert_eq!(decrypt_dave(&ctx, 2, &frame(&DAVE_MARKER)), None);
        assert_eq!(decrypt_dave(&ctx, 2, &frame(&[])), Some(frame(&[])));
    }
}
