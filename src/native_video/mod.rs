//! Native Discord video (camera + Go Live) for Litecord.
//!
//! This module is **additive**: the original `LTPV` peer-to-peer stack keeps
//! working exactly as before.  Native video is the path used when talking to
//! official Discord clients:
//!
//! * camera shares the parent voice connection (audio SSRC family + video SSRCs),
//! * Go Live uses a separate voice connection created from the main gateway,
//! * media is RTP/H.264, protected by transport encryption and DAVE.
//!
//! Only the receive side lives here for now; the existing capture/encode path
//! stays in `screen_capture` and is reused by the future transmit phases.

pub mod crypto;
pub mod h264;
pub mod jitter;
pub mod receiver;
pub mod rtp;
pub mod rtcp;
pub mod sender;
pub mod stream;

pub use sender::{is_broadcasting, offer_outbound_frame, stop_sender};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use slint::{Rgba8Pixel, SharedPixelBuffer};

pub use receiver::{NativeVideoContext, NativeVideoSession};

/// Callback used to hand decoded frames to the same UI path as `LTPV`.
pub type FrameCallback =
    Arc<dyn Fn(u64, String, String, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static>;

static FRAME_CALLBACK: Mutex<Option<FrameCallback>> = Mutex::new(None);

/// Starting nonce for outbound RTCP on a **receive-only** connection (Go Live),
/// which has no audio sender sharing the key.  The parent connection instead
/// shares the audio counter, so both stay on one sequence.
pub fn new_rtcp_counter() -> Arc<std::sync::atomic::AtomicU32> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Arc::new(std::sync::atomic::AtomicU32::new(0x8000_0000 | (nanos & 0x3fff_ffff)))
}

/// Installs the UI frame sink.  `main` points this at the existing stream
/// preview/popout code, so native and `LTPV` frames share one renderer.
pub fn set_frame_callback(callback: FrameCallback) {
    if let Ok(mut guard) = FRAME_CALLBACK.lock() {
        *guard = Some(callback);
    }
}

pub(crate) fn emit_frame(
    user_id: u64,
    username: String,
    quality: String,
    frame: SharedPixelBuffer<Rgba8Pixel>,
) {
    let callback = FRAME_CALLBACK
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().cloned());
    if let Some(callback) = callback {
        callback(user_id, username, quality, frame);
    }
}

/// A video SSRC advertised by a peer through voice opcode 12.
#[derive(Debug, Clone)]
pub struct VideoSsrc {
    pub user_id: u64,
    pub ssrc: u32,
    pub rtx_ssrc: u32,
    pub rid: String,
    pub quality: u32,
    /// True for the SSRCs of a Go Live stream connection.
    pub stream: bool,
}

static VIDEO_SSRCS: Mutex<Option<HashMap<u32, VideoSsrc>>> = Mutex::new(None);

fn with_map<R>(f: impl FnOnce(&mut HashMap<u32, VideoSsrc>) -> R) -> R {
    let mut guard = VIDEO_SSRCS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// Registers the video/RTX SSRCs of a user (voice opcode 12).
pub fn register_user_video(
    user_id: u64,
    video_ssrc: u32,
    rtx_ssrc: u32,
    streams: &[(String, u32, u32, u32)],
    stream_connection: bool,
) {
    with_map(|map| {
        if video_ssrc != 0 {
            map.insert(
                video_ssrc,
                VideoSsrc {
                    user_id,
                    ssrc: video_ssrc,
                    rtx_ssrc,
                    rid: "100".to_string(),
                    quality: 100,
                    stream: stream_connection,
                },
            );
        }
        if rtx_ssrc != 0 {
            map.insert(
                rtx_ssrc,
                VideoSsrc {
                    user_id,
                    ssrc: video_ssrc,
                    rtx_ssrc,
                    rid: "100".to_string(),
                    quality: 100,
                    stream: stream_connection,
                },
            );
        }
        for (rid, quality, ssrc, rtx) in streams {
            if *ssrc != 0 {
                map.insert(
                    *ssrc,
                    VideoSsrc {
                        user_id,
                        ssrc: *ssrc,
                        rtx_ssrc: *rtx,
                        rid: rid.clone(),
                        quality: *quality,
                        stream: stream_connection,
                    },
                );
            }
            // RTX has its own SSRC; registering it lets the receiver spot
            // retransmissions and map them back to the primary stream.
            if *rtx != 0 {
                map.insert(
                    *rtx,
                    VideoSsrc {
                        user_id,
                        ssrc: *ssrc,
                        rtx_ssrc: *rtx,
                        rid: rid.clone(),
                        quality: *quality,
                        stream: stream_connection,
                    },
                );
            }
        }
    });
}

/// Removes every SSRC of a user that stopped sending camera/stream video.
pub fn unregister_user_video(user_id: u64) {
    with_map(|map| map.retain(|_, entry| entry.user_id != user_id));
}

/// Removes only the SSRCs that belong to one kind of connection, so the parent
/// voice connection and a Go Live viewer cannot erase each other's entries.
pub(crate) fn unregister_user_video_of(user_id: u64, stream_connection: bool) {
    with_map(|map| map.retain(|_, entry| entry.user_id != user_id || entry.stream != stream_connection));
}

/// Whether `ssrc` belongs to a video stream (and not to Opus audio).
pub fn is_video_ssrc(ssrc: u32) -> bool {
    with_map(|map| map.contains_key(&ssrc))
}

/// Owner of a video SSRC, if it was announced.
pub fn user_for_video_ssrc(ssrc: u32) -> Option<u64> {
    with_map(|map| map.get(&ssrc).map(|entry| entry.user_id))
}

/// Full registration of a video SSRC (primary or RTX), if it was announced.
pub fn video_info(ssrc: u32) -> Option<VideoSsrc> {
    with_map(|map| map.get(&ssrc).cloned())
}

/// Primary SSRCs the local client should ask for with opcode 15.
pub fn wanted_ssrcs() -> Vec<u32> {
    with_map(|map| {
        map.values()
            .filter(|entry| entry.ssrc != 0)
            .map(|entry| entry.ssrc)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    })
}

/// Drops every SSRC (voice call ended).
pub fn clear_video_ssrcs() {
    with_map(|map| map.clear());
}

/// Applies a voice gateway opcode 12 payload (video state).
///
/// Registers the camera/Go Live SSRCs, fills `ssrc_to_uid` (shared with the
/// audio path) and returns the **primary** SSRCs that should be requested with
/// opcode 15.  When the payload announces no video at all, the user's state is
/// cleared and the result is empty.
pub fn handle_video_state(
    data: &serde_json::Value,
    fallback_owner: u64,
    stream_connection: bool,
    ssrc_to_uid: &Arc<Mutex<HashMap<u32, u64>>>,
) -> Vec<u32> {
    let user_id = data["user_id"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| data["user_id"].as_u64())
        .filter(|id| *id != 0)
        .unwrap_or(fallback_owner);
    if user_id == 0 {
        return Vec::new();
    }

    let audio_ssrc = data["audio_ssrc"].as_u64().unwrap_or(0) as u32;
    let video_ssrc = data["video_ssrc"].as_u64().unwrap_or(0) as u32;
    let rtx_ssrc = data["rtx_ssrc"].as_u64().unwrap_or(0) as u32;

    let mut streams = Vec::new();
    if let Some(list) = data["streams"].as_array() {
        for stream in list {
            let ssrc = stream["ssrc"].as_u64().unwrap_or(0) as u32;
            let rtx = stream["rtx_ssrc"].as_u64().unwrap_or(0) as u32;
            if ssrc == 0 && rtx == 0 {
                continue;
            }
            streams.push((
                stream["rid"].as_str().unwrap_or("100").to_string(),
                stream["quality"].as_u64().unwrap_or(100) as u32,
                ssrc,
                rtx,
            ));
        }
    }

    if video_ssrc == 0 && streams.is_empty() {
        // Video stopped (or never started) on this connection.  Keep the user's
        // audio SSRC mapped: losing it would break the speaking indicator and
        // audio attribution.
        unregister_user_video_of(user_id, stream_connection);
        if let Ok(mut map) = ssrc_to_uid.lock() {
            map.retain(|ssrc, uid| *uid != user_id || (audio_ssrc != 0 && *ssrc == audio_ssrc));
            if audio_ssrc != 0 {
                map.insert(audio_ssrc, user_id);
            }
        }
        // A Go Live viewer may still be sending this user's video.
        if !crate::native_video::stream::has_viewer_for(user_id) {
            crate::screen_capture::types::set_peer_streaming(user_id, false);
        }
        return Vec::new();
    }

    register_user_video(user_id, video_ssrc, rtx_ssrc, &streams, stream_connection);
    if let Ok(mut map) = ssrc_to_uid.lock() {
        if audio_ssrc != 0 {
            map.insert(audio_ssrc, user_id);
        }
        if video_ssrc != 0 {
            map.insert(video_ssrc, user_id);
        }
        if rtx_ssrc != 0 {
            map.insert(rtx_ssrc, user_id);
        }
        for (_, _, ssrc, rtx) in &streams {
            if *ssrc != 0 {
                map.insert(*ssrc, user_id);
            }
            if *rtx != 0 {
                map.insert(*rtx, user_id);
            }
        }
    }

    let mut primary: Vec<u32> = Vec::new();
    if video_ssrc != 0 {
        primary.push(video_ssrc);
    }
    for (_, _, ssrc, _) in &streams {
        if *ssrc != 0 && !primary.contains(ssrc) {
            primary.push(*ssrc);
        }
    }
    primary
}

/// Builds the `d` object of a voice opcode 15 (Media Sink Wants) packet.
pub fn media_sink_wants(primary_ssrcs: &[u32]) -> serde_json::Value {
    let mut wants = serde_json::Map::new();
    let mut pixel_counts = serde_json::Map::new();
    for ssrc in primary_ssrcs {
        wants.insert(ssrc.to_string(), serde_json::json!(100));
        pixel_counts.insert(ssrc.to_string(), serde_json::json!(1920 * 1080));
    }
    if !wants.is_empty() {
        wants.insert("any".to_string(), serde_json::json!(50));
        wants.insert("pixelCounts".to_string(), serde_json::Value::Object(pixel_counts));
    }
    serde_json::Value::Object(wants)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn video_state_registers_ssrcs_and_builds_the_wants() {
        let map = Arc::new(Mutex::new(HashMap::new()));
        let data = json!({
            "user_id": "42",
            "audio_ssrc": 424242,
            "video_ssrc": 424243,
            "rtx_ssrc": 424244,
            "streams": [
                { "rid": "100", "ssrc": 424245, "rtx_ssrc": 424246, "quality": 100 }
            ]
        });

        let wants = handle_video_state(&data, 0, false, &map);
        assert_eq!(wants, vec![424243, 424245]);
        assert_eq!(user_for_video_ssrc(424246), Some(42));

        let shared = map.lock().unwrap();
        assert_eq!(shared.get(&424242), Some(&42));
        assert_eq!(shared.get(&424244), Some(&42));
        drop(shared);

        let sink = media_sink_wants(&wants);
        assert_eq!(sink["424243"], 100);
        assert_eq!(sink["pixelCounts"]["424243"], 1920 * 1080);
        assert_eq!(sink["any"], 50);
    }

    #[test]
    fn stopping_video_keeps_the_audio_mapping() {
        let map = Arc::new(Mutex::new(HashMap::new()));
        let with_video = json!({
            "user_id": 424250,
            "audio_ssrc": 424251,
            "video_ssrc": 424252,
            "streams": []
        });
        handle_video_state(&with_video, 0, false, &map);
        assert_eq!(user_for_video_ssrc(424252), Some(424250));

        let stopped = json!({
            "user_id": 424250,
            "audio_ssrc": 424251,
            "video_ssrc": 0,
            "streams": []
        });
        assert!(handle_video_state(&stopped, 0, false, &map).is_empty());
        assert_eq!(user_for_video_ssrc(424252), None);

        let shared = map.lock().unwrap();
        assert_eq!(shared.get(&424251), Some(&424250));
        assert_eq!(shared.get(&424252), None);
    }

    #[test]
    fn stopping_camera_keeps_the_go_live_ssrcs() {
        let map = Arc::new(Mutex::new(HashMap::new()));
        let camera = json!({
            "user_id": 424260,
            "audio_ssrc": 424261,
            "video_ssrc": 424262,
            "streams": []
        });
        handle_video_state(&camera, 0, false, &map);

        let go_live = json!({
            "user_id": 424260,
            "audio_ssrc": 424261,
            "video_ssrc": 424263,
            "streams": [{ "rid": "100", "ssrc": 424264, "rtx_ssrc": 424265, "quality": 100 }]
        });
        handle_video_state(&go_live, 424260, true, &map);
        assert_eq!(user_for_video_ssrc(424264), Some(424260));

        // Camera off on the parent connection must not erase the Go Live SSRCs.
        let camera_off = json!({
            "user_id": 424260,
            "audio_ssrc": 424261,
            "video_ssrc": 0,
            "streams": []
        });
        handle_video_state(&camera_off, 0, false, &map);
        assert_eq!(user_for_video_ssrc(424262), None);
        assert_eq!(user_for_video_ssrc(424263), Some(424260));
        assert_eq!(user_for_video_ssrc(424264), Some(424260));

        // And the Go Live viewer ending only removes its own entries.
        unregister_user_video_of(424260, true);
        assert_eq!(user_for_video_ssrc(424263), None);
        assert_eq!(user_for_video_ssrc(424264), None);
        let shared = map.lock().unwrap();
        assert_eq!(shared.get(&424261), Some(&424260));
    }
}
