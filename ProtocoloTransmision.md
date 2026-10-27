# Native Discord Video in Litecord — Reverse Engineering & Implementation Plan

**Status:** Research complete, implementation **not started**.
**Repository:** `/home/kes/Litecord`
**Audited commit:** `d62ad91` on branch `build/unified-local-release` (working tree mid-merge, 2026‑10‑27).
**Scope chosen:** receive other people's **camera** and **Go Live** (DM calls and guild voice channels), and transmit our own **camera** and **desktop** as a normal Discord client does.
**Document language:** English (per request).
**Resume trigger:** the user says *"haz esto"* / *"seguimos con ProtocoloTransmision.md"* after the pending merge conflicts are resolved.

> ⚠️ **Read this first.** This document is the single source of truth for the work. It contains:
> 1. what Litecord does today for video (it is **not** Discord video),
> 2. the reverse-engineered Discord protocol (gateway, RTP, DAVE, streams),
> 3. a precise gap analysis against the current code,
> 4. the target module design and a phased implementation plan,
> 5. the test plan, open questions, and exact resume instructions.
>
> Do not start coding before the merge is finished: the tree currently has unresolved `UU` conflicts in
> `src/gateway/voice.rs`, `src/main.rs`, `src/http.rs`, `src/audio/dsp.rs`, `src/screen_capture/mod.rs`,
> `src/ui/data_loader.rs`, `src/utils/mod.rs`, `ui/components/settings_view.slint`.

---

## 0. TL;DR

Litecord already has a **complete, working, private video stack** — but it is *Litecord‑to‑Litecord only*.
It captures, encodes H.264, encrypts, packetizes over raw UDP with its own `LTPV` protocol, signals through a
Cloudflare Worker, and renders remote frames in Slint. Official Discord clients have no idea this exists.

Native Discord interoperability is **absent in every layer**:

| Layer | Litecord today | Discord native |
|---|---|---|
| Voice gateway version | `?v=4` | `v5+` for video, `v9` recommended |
| Identify | `"video": false, "streams": []` | `"video": true` + stream descriptors + `channel_id` |
| Codec advertisement | none | `codecs[]` in Select Protocol (Opus + H264 + RTX) |
| Session Description | reads `secret_key` only | also `video_codec`, `media_session_id`, `dave_protocol_version` |
| Inbound UDP | every packet treated as Opus/DAVE audio | RTP + RTCP demux, H264 depacketization, RTX/NACK, DAVE video |
| Camera | only Litecord `LTPV` frames | voice opcode 12 + video RTP on the same UDP socket |
| Go Live | only Litecord `LTPV` frames | separate stream RTC connection + opcode 15 subscriptions |
| Watch signalling | voice opcode 20 (wrong socket, wrong key for DMs) | **main** gateway opcode 20 + STREAM_CREATE/STREAM_SERVER_UPDATE |
| DAVE | audio only (`MediaType::AUDIO`) | audio **and video** (`MediaType::VIDEO`, `Codec::H264`) |

The shortest correct mental model:

- **Camera** = a second SSRC family on the *existing* voice UDP connection. Enable video, map SSRCs via opcode 12, negotiate H264, send/receive RTP.
- **Go Live** = a *second voice connection* (its own WebSocket + UDP socket + SSRCs + DAVE group) requested from the **main** gateway with opcode 18, watched with opcode 20.

The implementation lands in a new `src/native_video/` module plus surgical changes in
`src/gateway/voice.rs`, `src/gateway/client.rs`, `src/gateway/types.rs`, and `src/main.rs`.
Reuse the existing H.264 decoder and UI frame callback; do **not** rewrite the encoder or the renderer.

---

## 1. What Litecord currently supports for viewing streams

This section answers the question *"¿qué tiene Litecord hoy para ver transmisiones?"* precisely.

### 1.1 The existing stack is `LTPV` (Litecord Transport Protocol for Video)

Litecord's video is a proprietary peer-to-peer protocol defined in `src/screen_capture/types.rs`:

```rust
pub const P2P_VIDEO_PORT: u16 = 50005;
pub const MAGIC: &[u8; 4] = b"LTPV";
pub const CHUNK_SIZE: usize = 1200;        // WAN-safe MTU
pub const OP_ANNOUNCE:    u8 = 1;
pub const OP_VIDEO_CHUNK: u8 = 2;
pub const OP_STOP:        u8 = 3;
pub const OP_HEARTBEAT:   u8 = 4;
pub const OP_AUDIO_FRAME: u8 = 5;
pub const OP_KEYFRAME_REQ:u8 = 6;
pub const OP_FEC_PARITY:  u8 = 7;
pub const OP_QOS_FEEDBACK:u8 = 8;
pub const OP_WATCH:       u8 = 9;
pub const OP_UNWATCH:     u8 = 10;
pub const OP_WATCH_ACK:   u8 = 11;
```

Pipeline:

1. **Capture** — desktop via DXGI Desktop Duplication / Windows Graphics Capture on Windows, PipeWire portal
   (GStreamer `pipewiresrc`) on Wayland/X11; webcam via the `cameras` crate (V4L2 on Linux, Media Foundation/DirectShow on Windows).
2. **Encode** — H.264 Annex B through `src/encoder/` (`NVENC`, `AMF`, `WMF`, OpenH264 fallback).
3. **Encrypt** — per-stream AES-256-GCM key, derived with X25519 and delivered in `OP_WATCH_ACK`
   (`src/screen_capture/crypto.rs`, `docs/WATCH_ON_DEMAND.md`).
4. **Packetize** — 1200-byte chunks, XOR FEC parity, sequence + frame ids, sent to viewers' UDP ports.
5. **Signal** — WebSocket to `wss://litecord-signal.ricosgames-henrique.workers.dev` (override with
   `LITECORD_SIGNALING_URL`), room keyed by the Discord **voice channel id**, protocol version 3.
6. **Receive** — `start_receiver()` reassembles chunks, decrypts, decodes, and calls the UI callback.
7. **Render** — Slint main view and `PopoutStreamWindow` (picture-in-picture), latest-frame store, GPU path.
8. **Extras** — watch-on-demand, browser viewers through `str0m` WebRTC (max 3), screen-share audio loopback,
   automation commands `watch_stream` / `unwatch_stream`.

### 1.2 What a user actually gets today

| Capability | Works today? | Notes |
|---|---|---|
| Litecord user shares desktop → another Litecord user watches | ✅ | Same signaling room, same protocol version 3 |
| Litecord user shares camera → another Litecord user watches | ✅ | Webcam capture exists; audio intentionally disabled for camera |
| Browser viewer via link | ✅ | `str0m`, H.264 passthrough, max 3 viewers |
| Watch only when requested (no wasted upload) | ✅ | `OP_WATCH` / `OP_UNWATCH`, key delivery |
| See a **Discord** friend's camera in a DM call | ❌ | No native video at all |
| See a **Discord** friend's Go Live in a guild | ❌ | No stream RTC connection |
| Discord friends see **Litecord's** camera / screen | ❌ | They never join the worker room |
| Native Discord camera in a guild voice channel | ❌ | `video: false`, no opcode 12 |
| Native Discord Go Live broadcast | ❌ | No opcode 18/20 on the main gateway |

### 1.3 Why the "Watch" button does nothing with Discord users

`src/gateway/client.rs` reacts to `VOICE_STATE_UPDATE`:

```rust
let is_streaming = v["d"]["self_stream"].as_bool().unwrap_or(false)
                || v["d"]["self_video"].as_bool().unwrap_or(false);
set_peer_streaming(event_uid, is_streaming);
if is_streaming {
    if let (Some(gid), Some(cid)) = (v["d"]["guild_id"].as_str(), event_chan.as_deref()) {
        let stream_key = format!("guild:{}:{}:{}", gid, cid, event_uid);
        crate::gateway::voice::watch_stream(&stream_key);
    }
}
```

Three independent problems:

1. `watch_stream()` sends **opcode 20 on the voice WebSocket** (`src/gateway/voice.rs`), but Stream Watch is a
   **main gateway** command. The voice server ignores or errors it.
2. The key is always `guild:…`. In a DM call there is no guild; the correct key is `call:{channel_id}:{owner_id}`.
3. Even if the command were accepted, Litecord never handles `STREAM_CREATE` / `STREAM_SERVER_UPDATE`, never opens
   the stream RTC connection, never subscribes with opcode 15, and has no H.264 RTP depacketizer — so no bytes
   would ever be decoded.

`self_stream` / `self_video` only tell you that the *state* changed. They are not media.

### 1.4 Current code map (what to reuse, what to replace)

| File | Role | Reuse? |
|---|---|---|
| `src/screen_capture/types.rs` | `LTPV` opcodes, SSRC/frame stores, `peer_is_streaming`, `get_active_stream_frames` | Reuse the frame store/UI plumbing; add native stream state alongside |
| `src/screen_capture/manager.rs` | Capture + `LTPV` sender/receiver, camera pipeline | Reuse capture/encode; keep `LTPV` for Litecord↔Litecord |
| `src/screen_capture/video_pipeline.rs` | `StreamDecoders`, `decode_video_frame()`, Annex B helpers, SPS/PPS cache | **Reuse as the native decoder bridge** |
| `src/screen_capture/signaling.rs` | Cloudflare Worker client, protocol 3 | Untouched by native work |
| `src/screen_capture/crypto.rs` | X25519 + AES-GCM for `LTPV` | Not used by Discord transport encryption (different scheme) |
| `src/screen_capture/web_share.rs` | `str0m` browser viewers | Untouched |
| `src/gateway/voice.rs` | Voice WS, DAVE audio, UDP audio loop | **Major changes** (v9, video, RTP demux) |
| `src/gateway/client.rs` | Main gateway events and commands | Add stream commands/events, fix watch trigger |
| `src/gateway/types.rs` | `GatewayEvent` enum | Add stream + native video events |
| `src/main.rs` | UI wiring, `start_receiver`, stream card | Add native frame delivery + stream lifecycle |
| `src/encoder/` | H.264 encoders | Reuse for camera/Go Live transmit |
| `src/screen_capture/sources/` | Screen capture backends | Reuse; macOS backend missing |

### 1.5 Platform status of the existing pipeline (relevant to the final goal)

- **Windows:** complete. DXGI/WGC capture, NVENC/AMF/WMF/OpenH264 encode, D3D11 decode, WASAPI loopback audio.
- **Linux:** Wayland portal + PipeWire via GStreamer, X11 fallback, V4L2 camera, ALSA/PipeWire audio.
  Screen capture works; GPU encode is Windows-only, so Linux encodes with OpenH264 (CPU) unless FFmpeg is present.
- **macOS:** **not implemented.** `src/screen_capture/sources/mod.rs` routes every non-Windows target to
  `linux_portal.rs`, which is `#[cfg(target_os = "linux")]`-gated. There is no ScreenCaptureKit/AVFoundation backend,
  no CoreAudio loopback, and `cameras` support is untested. Shipping macOS screen share requires a new backend
  (Phase 6, separate track from the Discord protocol work).

---

## 2. Reverse-engineered Discord video architecture

Sources: official Discord voice docs, the DAVE whitepaper (`daveprotocol.com`), and the
`discord-userdoccers` protocol reference. Local copies of the reference pages used here:
`/tmp/opencode/discord-docs/voice-connections.mdx`, `/tmp/opencode/discord-docs/gateway-events.mdx`.

### 2.1 Two transports, one mental model

Discord media always travels over RTP on UDP (the "UDP" voice protocol; WebRTC is an alternative transport we do
**not** need). What changes is *which connection* carries it:

```
                 MAIN GATEWAY (wss://gateway.discord.gg)
                 ├─ VOICE_STATE_UPDATE / VOICE_SERVER_UPDATE  → parent voice connection
                 ├─ op 18 Create Stream  ─┐
                 ├─ op 20 Watch Stream   ─┼→ STREAM_CREATE + STREAM_SERVER_UPDATE
                 ├─ op 19 Delete Stream  ─┤
                 └─ op 22 Set Stream Paused ─┘
                                              │
        ┌─────────────────────────────────────┴─────────────────────────────┐
        │                                                                   │
  PARENT VOICE CONNECTION                                       STREAM VOICE CONNECTION
  (one per voice channel / DM call)                             (one per Go Live stream)
  WS  wss://<endpoint>/?v=9                                     WS  wss://<stream endpoint>/?v=9
  UDP <ip>:<port>                                               UDP <stream ip>:<port>
  ├─ microphone  (Opus,   ssrc from Ready)                      ├─ screen video (H264, ssrc from stream Ready)
  ├─ camera      (H264,   ssrc+rtx from Ready.streams)          ├─ system/application audio (Opus, SOUNDSHARE)
  └─ DAVE group = channel_id                                    └─ DAVE group = media_session_id
```

Key rules:

- Camera and microphone share the **parent** connection and its `secret_key`, DAVE session, and UDP socket.
- Go Live never uses the parent connection's SSRCs. It has its own namespace, token, endpoint, `secret_key`, and DAVE group.
- The parent voice connection **must stay connected** while watching/streaming Go Live.
- Viewers only receive; never transmit on a stream connection they are watching.

### 2.2 Voice gateway handshake (v9)

1. Main gateway sends `VOICE_STATE_UPDATE` (with `session_id`) and `VOICE_SERVER_UPDATE` (with `token`, `endpoint`, `guild_id`).
2. Open `wss://<endpoint>/?v=9`. (v5 is the minimum for Media Sink Wants; v9 adds `channel_id` in Identify.)
3. Server sends **op 8 Hello**: `{ "heartbeat_interval": 41250 }`.
4. Client sends **op 0 Identify**:

```json
{
  "op": 0,
  "d": {
    "server_id": "41771983423143937",
    "channel_id": "127121515262115840",
    "user_id": "104694319306248192",
    "session_id": "30f32c5d54ae86130fc4a215c7474263",
    "token": "66d29164ee8cd919",
    "video": true,
    "streams": [
      { "type": "video", "rid": "100", "quality": 100 }
    ],
    "max_dave_protocol_version": 1
  }
}
```

  - `channel_id` is required on v9+ (for DM calls it is the DM channel id).
  - `video: true` is what makes the server willing to route video state.
  - `streams` describes the local simulcast layers we can encode; the server assigns SSRCs in Ready.
  - `max_dave_protocol_version: 1` enables DAVE.

5. Server sends **op 2 Ready**:

```json
{
  "op": 2,
  "d": {
    "ssrc": 12871,
    "ip": "127.0.0.1",
    "port": 1234,
    "modes": ["aead_aes256_gcm_rtpsize", "aead_xchacha20_poly1305_rtpsize"],
    "experiments": ["fixed_keyframe_interval"],
    "streams": [
      { "type": "video", "ssrc": 12872, "rtx_ssrc": 12873, "rid": "50",  "quality": 50,  "active": false },
      { "type": "video", "ssrc": 12874, "rtx_ssrc": 12875, "rid": "100", "quality": 100, "active": false }
    ]
  }
}
```

  - `ssrc` is the **audio** SSRC (microphone), also used for RTCP SR.
  - `streams[]` are the local video SSRCs. If omitted for a negotiated video stream, RTX SSRC is `ssrc + 1`.

6. UDP socket → **IP discovery** (type `0x0001`, length `70`, SSRC, 64-byte null-terminated address, 2-byte port;
   response type `0x0002`), then **op 1 Select Protocol**.
7. Server sends **op 4 Session Description** with `mode`, `secret_key[32]`, `audio_codec`, `video_codec`,
   `media_session_id`, `dave_protocol_version`.
8. Client sends **op 5 Speaking** (`speaking: 1`) before audible Opus.
9. Media flows. Heartbeat op 3 with `t` + `seq_ack`; server replies op 6.

### 2.3 Select Protocol and codec negotiation (the part Litecord omits)

```json
{
  "op": 1,
  "d": {
    "protocol": "udp",
    "data": {
      "address": "203.0.113.7",
      "port": 54321,
      "mode": "aead_aes256_gcm_rtpsize"
    },
    "codecs": [
      { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120 },
      { "name": "H264", "type": "video", "priority": 1000,
        "payload_type": 103, "rtx_payload_type": 104,
        "encode": true, "decode": true }
    ],
    "rtc_connection_id": "<uuid>",
    "experiments": ["fixed_keyframe_interval"]
  }
}
```

Facts that matter:

- Payload types are **client-chosen dynamic values**; `96` is reserved for probe packets.
- If `codecs` is absent, the server assumes Opus/120 and **no video codec** (`video_codec` is then absent).
- Codec selection: the server prefers AV1/H265 when both sides support them, defaults to **H264**, and falls back
  to H264 if no common codec exists. To force H264, advertise **only H264** (decode=true) and do not advertise
  AV1/H265. Advertising VP8 as decode-only is optional and only helps legacy senders.
- `video_codec` in Session Description tells us what **we should send**; other participants may still send a
  different codec we declared we can decode. Our decoder only needs H264 (and the plan is H264-only).

### 2.4 Transport encryption (applies to audio, video, and RTCP)

Two modern modes, both "RTP size":

| Mode | Cipher | Nonce |
|---|---|---|
| `aead_aes256_gcm_rtpsize` | AES-256-GCM | 4-byte incremental counter appended to the packet |
| `aead_xchacha20_poly1305_rtpsize` | XChaCha20-Poly1305 | 4-byte incremental counter appended to the packet |

Rules:

- The 4-byte nonce suffix is **not** part of the RTP payload: strip it, decrypt, and treat the remaining bytes as RTP/RTCP.
- AAD (authenticated but not encrypted) is the clear RTP header: fixed 12 bytes + CSRCs + the 4-byte RTP extension preamble.
  Individual header-extension elements are encrypted with the payload.
- The same `secret_key` from Session Description protects everything on that connection (outbound and inbound).
- The counter is per-connection, incremented per packet; implementations commonly start from a random 32-bit value.
  Replay protection should reject duplicate nonces.
- Legacy `xsalsa20_poly1305*` modes exist but are deprecated; do not implement them.

Litecord's existing audio decrypt (`src/gateway/voice.rs`, around the `Aes256Gcm` block) already does the
suffix/AAD dance for Opus. The native video work must factor that out into a reusable `TransportDecryptor`
instead of duplicating it.

### 2.5 RTP / RTCP on the wire

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|V=2|P|X|  CC   |M|     PT      |       sequence number         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           timestamp                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|           synchronization source (SSRC) identifier            |
+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
|            contributing source (CSRC) identifiers             |
|                             ....                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    header extension (0xBEDE)                  |
+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
|                     encrypted payload ...                     |
```

- Version 2. Marker bit (`0x80` in the PT byte) = last packet of a video frame.
- Video timestamp clock: **90 kHz**. Increment by `90000 / fps`.
- Audio: Opus, typically PT 120, timestamp advances by 960 per 20 ms.
- RTCP is multiplexed on the same socket: SR `200`, RR `201`, RTPFB `205` (NACK), PSFB `206` (PLI/FIR).
- RTCP is protected by the same transport key; for rtpsize modes the RTCP header stays clear as AAD.

Header extensions (one-byte profile `0xBEDE`):

| ID | URI | Use |
|---|---|---|
| 1 | `ssrc-audio-level` | audio level / VAD |
| 2 | `toffset` | video timestamp offset |
| 3 | `abs-send-time` | congestion control |
| 4 | `video-orientation` | camera rotation |
| 5 | `transport-wide-cc` | congestion control |
| 6 | `playout-delay` | expected on video, Discord uses it |
| 7 | `video-content-type` | normal video vs screen content |
| 8 | `video-timing` | encode/decode timing |
| 9 | `discord speaker` | speaking flags |
| 10 | `sdes:mid` | media section |
| 11 | `sdes:rtp-stream-id` | RID (`100`, `50`, …) |
| 12 | `sdes:repaired-rtp-stream-id` | RTX RID |

Our parser must tolerate unknown extensions and must be able to locate the payload regardless of them.

### 2.6 Camera path (the "normal Discord video" case)

**Transmit**

1. Connect with `video: true`, `streams: [{type:"video", rid:"100", quality:100}]`.
2. From Ready, take the matching stream's `ssrc` and `rtx_ssrc`.
3. Send **op 12 Video** to announce local state:

```json
{
  "op": 12,
  "d": {
    "audio_ssrc": 12871,
    "video_ssrc": 12874,
    "rtx_ssrc": 12875,
    "streams": [
      { "type": "video", "rid": "100", "ssrc": 12874, "active": true,
        "quality": 100, "rtx_ssrc": 12875,
        "max_bitrate": 2500000, "max_framerate": 30,
        "max_resolution": { "type": "fixed", "width": 1280, "height": 720 } }
    ]
  }
}
```

4. Encode H.264 → **DAVE encrypt the complete encoded frame** (`MediaType::VIDEO`, `Codec::H264`) → packetize
   (single NAL / STAP-A / FU-A) → transport-encrypt each RTP packet → send.
5. Set the marker bit on the final packet of each frame.
6. Send RTCP Sender Reports every ~5 s; retransmit NACKed packets as RTX; send a keyframe on PLI/FIR.
7. On pause/resume, send op 12 with `active: false/true`; on stop, send op 12 with `video_ssrc: 0` and an empty `streams`.

**Receive**

1. On every **inbound op 12**, cache:
   - `user_id` → `audio_ssrc`, `video_ssrc`,
   - each `streams[i].ssrc` → (user, rid, quality, max_bitrate/framerate/resolution),
   - each `streams[i].rtx_ssrc` → same stream.
2. Send **op 15 Media Sink Wants** to actually receive:

```json
{ "op": 15, "d": { "12874": 100, "any": 50, "pixelCounts": { "12874": 921600.0 } } }
```

   - Keys are **primary** SSRCs, never RTX.
   - `0` disables a stream; `100` is the highest layer.
   - Update on view open/close/resize/pin/background.
3. Demux UDP: RTCP first (packet types 200/201/205/206), otherwise RTP.
4. Transport-decrypt → parse RTP → route by SSRC using the op 12 map.
5. Reorder by sequence number, detect gaps, send Generic NACK (RTCP RTPFB) and request a keyframe with PLI when the
   decoder has no reference frame.
6. Depacketize H264 to Annex B (single NAL, STAP-A, FU-A), **then** DAVE-decrypt the frame, then decode.
7. Feed the decoded RGBA frame into the existing `get_active_stream_frames()` / UI callback.

**Opcode 12 is bidirectional and easy to confuse with "user joined".** Opcode 11 is Clients Connect; opcode 12 is Video;
opcode 13 is Client Disconnect. Sending video without a prior op 12 (or receiving without a map) yields
`Invalid SSRC` disconnects.

### 2.7 Go Live path

**Start broadcasting**

1. Must already be connected to the parent voice channel.
2. Main gateway **op 18 Create Stream**:

```json
{ "op": 18, "d": { "type": "guild", "guild_id": "839502008108580904",
                   "channel_id": "850360749460553769", "preferred_region": "us-east" } }
```

   For a DM call: `"type": "call"`, `"guild_id": null`, `"channel_id": "<dm channel id>"`.

3. Receive **STREAM_CREATE** (stream object: `stream_key`, `rtc_server_id`, `rtc_channel_id`, `region`, `paused`)
   and **STREAM_SERVER_UPDATE** (`token`, `stream_key`, `endpoint`).
   A failure arrives as **STREAM_DELETE** with a `reason`.
4. Open a **new** voice WebSocket to `wss://<stream endpoint>/?v=9` and Identify with:
   - `server_id` = `rtc_server_id`
   - `channel_id` = `rtc_channel_id`
   - `session_id` = the main gateway session id
   - `token` = stream token
   - `video: true`, `streams: [{type:"video", rid:"100", quality:100}]`
5. Ready → its own audio SSRC + video SSRC/RTX. IP discovery → Select Protocol (same codecs) → Session Description
   → own `secret_key` and `media_session_id`.
6. DAVE group for the stream connection = **stream-specific** MLS group id. Per protocol docs, use the
   `media_session_id` (one less than `rtc_server_id`). Do not reuse the channel id.
7. Send op 12 on the **stream** connection, then H264 video RTP. System/application audio goes on the same stream
   connection as Opus with the `SOUNDSHARE` speaking flag (not `VOICE`).
8. Pause/resume with main gateway **op 22 Set Stream Paused**; stop with **op 19 Delete Stream**.

**Watch a stream**

1. Send main gateway **op 20 Watch Stream**:

```json
{ "op": 20, "d": { "stream_key": "call:1110739331624210483:852892297661906993" } }
```

2. Receive STREAM_CREATE + STREAM_SERVER_UPDATE (or STREAM_DELETE on failure).
3. Connect to the stream RTC server exactly as above (same Identify shape, `video: true`, no local streams needed
   for a pure viewer — but `video: true` is still required to receive).
4. After Session Description, send **op 15 Media Sink Wants** for the streamer's SSRCs. The streamer's SSRCs are
   learned from the **stream connection's op 12** (the stream voice gateway sends video state for the owner).
5. Receive → transport-decrypt → depacketize → DAVE-decrypt → decode → render.
6. **Never send media** on a stream connection you are only watching.
7. On STREAM_DELETE / `unavailable: true`, tear down only the stream connection; the parent voice call stays up.

**Stream keys**

| Context | Format | Example |
|---|---|---|
| Guild | `guild:{guild_id}:{channel_id}:{owner_id}` | `guild:839502008108580904:850360749460553769:852892297661906993` |
| DM call | `call:{channel_id}:{owner_id}` | `call:1110739331624210483:852892297661906993` |
| Speed test | `test:{owner_id}` | not used in normal flows |

Litecord currently always builds the guild form, which can never match a DM stream.

### 2.8 DAVE for video

DAVE is applied to **encoded frames**, after depacketization on receive and before packetization on send.
Transport encryption still wraps every RTP packet; they are independent layers.

- The same MLS group and sender keys protect audio and video; the media type is passed to the cryptor.
- `davey 0.1.4` supports it directly:

```rust
// receive (after H264 depacketization, Annex B frame)
let plain = dave_session.decrypt(sender_user_id, MediaType::VIDEO, &encoded_frame)?;

// send (before H264 packetization)
let encrypted = dave_session.encrypt(MediaType::VIDEO, Codec::H264, &annex_b_frame)?;
```

- H264 specifics from the DAVE whitepaper: VCL NAL payloads are fully encrypted; non-VCL NAL headers/metadata that
  the packetizer/depacketizer must read are left clear; 3-byte start codes in clear ranges become 4-byte start codes;
  the encoder retries if a start-code sequence appears in ciphertext. Consequence for us:
  **DAVE operates on the full encoded frame, not on individual RTP payloads.**
- Passthrough mode is automatic: unencrypted frames pass through (e.g. before the first transition).
- DAVE transition opcodes are already implemented for audio (21/22 prepare/execute, 23 ready, 24 epoch, 25 external
  sender, 26 key package, 27 proposals, 28 commit/welcome). The **same session** must be reused for video; only the
  `MediaType` changes. For a stream connection, a **new** `DaveSession` must be created with the stream group id.

### 2.9 End-to-end sequence: camera in a DM call

```
Litecord                     Main GW                 Voice GW                    SFU/UDP
   │  VOICE_STATE_UPDATE ───────▶│
   │  (guild_id=null, channel)   │
   │◀── VOICE_SERVER_UPDATE ─────│ (token, endpoint)
   │◀── VOICE_STATE_UPDATE ──────│ (session_id)
   │                                                        │
   │  wss://endpoint/?v=9 ─────────────────────────────────▶│
   │◀────────────────────────────────────────── op 8 Hello │
   │  op 0 Identify {video:true, streams:[…], channel_id} ─▶│
   │◀──────────────────────────── op 2 Ready {ssrc,streams}│
   │  UDP IP discovery ────────────────────────────────────────────────▶│
   │  op 1 Select Protocol {codecs:[opus,H264]} ───────────▶│
   │◀──────────────── op 4 Session Description {key,codec} │
   │  op 5 Speaking ───────────────────────────────────────▶│
   │  op 12 Video {video_ssrc, rtx_ssrc, active} ──────────▶│
   │◀──────────────────────── op 12 Video {user_id, ssrcs} │  (for each peer)
   │  op 15 Media Sink Wants {ssrc:100} ───────────────────▶│
   │◀──────────────────────────────────────────── H264 RTP ────────────│
   │  decrypt → reorder → depacketize → DAVE → decode → UI │
   │  H264 RTP (our camera) ───────────────────────────────────────────▶│
```

### 2.10 End-to-end sequence: Go Live in a guild

```
Litecord            Main GW                  Stream GW                    Stream SFU
   │ op 20 Watch Stream ▶│
   │◀─ STREAM_CREATE ────│ {rtc_server_id, rtc_channel_id}
   │◀─ STREAM_SERVER_UPDATE {token, endpoint}
   │  new WS to endpoint, Identify {server_id=rtc_server_id,
   │     channel_id=rtc_channel_id, video:true, token} ────────────────▶│
   │◀───────────────────────────────────────────── op 2 Ready {ssrcs}  │
   │  IP discovery → op 1 Select Protocol → op 4 Session Description   │
   │  op 12 Video (owner SSRCs arrive) ◀───────────────────────────────│
   │  op 15 Media Sink Wants {owner_ssrc:100} ─────────────────────────▶│
   │◀──────────────────────────────────────────────────── H264 RTP ────│
   │  parent voice connection stays untouched                          │
```

---

## 3. Gap analysis: Litecord code vs the protocol

Line numbers are from commit `d62ad91` and will drift; use the symbol names.

### 3.1 Voice gateway session (`src/gateway/voice.rs`)

| Finding | Location | Impact |
|---|---|---|
| Connects with `?v=4` | `connect_voice_gateway`, URL construction | No Media Sink Wants (needs v5+), no required `channel_id` semantics (v9) |
| Identify sends `"video": false`, `"streams": []`, no `channel_id` | op 8 Hello handler | Server never routes video; op 12 may be rejected |
| Select Protocol has **no `codecs` array** | after IP discovery | Server assumes Opus only, no video codec; we never advertise H264 decode |
| Session Description ignores `video_codec`, `media_session_id`, `dave_protocol_version` | op 4 handler | We cannot know the send codec, stream DAVE group, or negotiated DAVE version |
| Every UDP packet is treated as encrypted Opus | receive loop | Video/RTCP packets are fed to Opus/DAVE-audio and dropped; possible log spam |
| DAVE decrypt hardcodes `MediaType::AUDIO` | receive loop | Video frames would fail even if parsed |
| No RTP header parser (fixed offsets only) | receive loop | CSRCs/extensions break payload offset; no SSRC routing beyond audio |
| No RTCP handling (SR/RR/NACK/PLI) | — | No loss repair, no keyframe requests, no quality feedback |
| No jitter/reorder buffer | — | Out-of-order video packets corrupt frames |
| No H264 depacketizer | — | Cannot turn RTP into a decodable frame |
| Opcode `11 | 12` handled together as "Client Connect" | ~line 1569 | Opcode 12 (Video) is misread as a user-join; `video_ssrc`, `streams`, `rtx_ssrc` are discarded. It may accidentally map `audio_ssrc`, which is not enough |
| Opcode 13 (Client Disconnect) unhandled | — | Stale SSRC map / participants |
| `watch_stream()` sends **op 20 over the voice WS** | `watch_stream`, channel drain task | Opcode 20 is a main-gateway command. Voice server ignores it. This is the root cause of "Watch does nothing" |
| DAVE session created with `channel_id` as group id | op 24 / session init | Correct for parent voice; wrong for Go Live stream connections (needs stream group id) |

### 3.2 Main gateway (`src/gateway/client.rs`, `src/gateway/types.rs`)

| Finding | Location | Impact |
|---|---|---|
| `VOICE_STATE_UPDATE` builds `guild:{gid}:{cid}:{uid}` unconditionally | ~line 775 | DM calls produce a key that cannot exist; `guild_id` is null there |
| Watch is triggered from `self_stream` **or** `self_video` without distinguishing camera vs Go Live | ~line 771 | Camera needs no op 20 at all; Go Live needs op 20 with the right key |
| No send path for main-gateway op 18/19/20/21/22 | `GatewayCommand` enum / WS write | Cannot create, watch, pause, or delete streams |
| No handling of `STREAM_CREATE`, `STREAM_SERVER_UPDATE`, `STREAM_UPDATE`, `STREAM_DELETE` | dispatch match | No stream endpoint/token, no lifecycle |
| `GatewayEvent` has no stream/video variants | `types.rs` | UI cannot react to native stream state |
| DM voice connect stores guild as `"@me"` | `VOICE_SERVER_UPDATE` handler | Fine for the parent connection `server_id`; must not be reused as a stream key |
| `voice_session_id` is kept | struct | Needed for stream Identify (`session_id`) — verify it holds the main GW session id, not the voice one |

### 3.3 UI / render path (`src/main.rs`, `src/screen_capture/`)

| Finding | Location | Impact |
|---|---|---|
| `start_receiver()` only starts the `LTPV` signaling/receiver | `screen_capture::manager` | No native frame source |
| `peer_is_streaming()` reflects `VOICE_STATE_UPDATE` flags only | `types.rs` | UI shows "streaming" but has no frames |
| `has_active_stream` is driven by `LTPV` frames | `main.rs` | Native streams never set it |
| `decode_video_frame()` expects Annex B and caches SPS/PPS per peer | `video_pipeline.rs` | Perfect bridge: call it with depacketized+DAVE-decrypted frames |
| Popout window/hide logic keyed by user id | `main.rs` | Reusable unchanged |
| No "watch camera" UI, only "watch stream" | `ui/appwindow.slint` | Camera should auto-display like Discord (no Watch button) |

### 3.4 Corrections to the earlier audit (`Correciones.md`)

The earlier note in `Correciones.md` §3 is directionally right (video is missing) but technically wrong in these points:

1. **"Send Opcode 20 to the voice gateway."** No. Opcode 20 is a **main gateway** command. Sending it on the voice
   WebSocket does nothing (and may be ignored/closed). Opcode numbers overlap between gateways; always check which socket.
2. **"The voice gateway assigns a video SSRC on opcode 20."** No. The **stream** server assigns SSRCs in its own Ready
   after `STREAM_CREATE` + `STREAM_SERVER_UPDATE`; the main gateway only allocates the stream.
3. **"Go Live video arrives on the existing UDP socket."** No. Go Live uses a **separate** voice connection and UDP socket.
   Camera is the one that shares the parent connection.
4. **DAVE covers audio only.** No. DAVE covers video too; `davey` supports `MediaType::VIDEO`/`Codec::H264`. The
   frame must be depacketized first; DAVE is not applied per RTP packet.
5. **"Pipe RTP H.264 into MPV."** RTP is not Annex B. It must be transport-decrypted, reordered, depacketized,
   DAVE-decrypted, and only then decoded (existing `decode_video_frame` or MPV).
6. **"Set `has_active_stream = true` when `self_stream` is true."** That only makes the UI lie. It must be driven by
   actual decoded frames (or at least by a successful stream subscription).

### 3.5 What is genuinely reusable

- `decode_video_frame()` + `StreamDecoders` (H.264, GPU/CPU, Annex B, SPS/PPS cache, IDR resync).
- `VideoEncoder` trait and all hardware encoders.
- Screen and camera capture backends (Windows/Linux).
- UI frame stores, popout, viewport handling, `has_active_stream`, participant model.
- `davey` session plumbing, especially the transition state machine.
- Audio path (Opus, jitter buffer, speaker queues); it must be split from video demux but not rewritten.
- AES-GCM decrypt code (refactor into a transport decryptor).

---

## 4. Target architecture

### 4.1 New module: `src/native_video/`

```text
src/native_video/
├── mod.rs                  // public API, lifecycle, event types
├── rtp.rs                  // RTP/RTCP header parse+build, extensions, validation
├── transport_crypto.rs     // aead_aes256_gcm_rtpsize + aead_xchacha20_poly1305_rtpsize
├── rtcp.rs                 // SR, RR, RTPFB(NACK), PSFB(PLI/FIR), feedback
├── jitter.rs               // per-SSRC reorder buffer + loss detection + NACK generation
├── depacketizer/
│   ├── mod.rs
│   └── h264.rs             // single NAL, STAP-A, FU-A → Annex B
├── packetizer/
│   ├── mod.rs
│   └── h264.rs             // Annex B NALs → RTP payloads, FU-A splitting, marker
├── ssrc_map.rs             // user_id ↔ audio/video/rtx SSRC, RID, quality
├── video_session.rs        // receive pipeline: UDP→decrypt→rtp→jitter→depacketize→DAVE→decoder
├── video_sender.rs         // capture frame → DAVE → packetize → transport encrypt → UDP
├── stream_conn.rs          // Go Live stream voice connection (WS + UDP + own DAVE)
└── tests/                  // fixtures + unit tests
```

### 4.2 Data flow (receive)

```
UDP socket
  │  (shared with audio; demux by first byte + packet type)
  ├─ RTCP (200/201/205/206) ──▶ transport decrypt ──▶ rtcp.rs ──▶ stats/NACK/PLI
  └─ RTP  (PT != opus)      ──▶ transport decrypt ──▶ rtp.rs
                                        │
                                        ├─ SSRC is video/rtx? ──▶ ssrc_map.rs
                                        ▼
                                   jitter.rs (per SSRC)
                                        │ ordered payloads
                                        ▼
                                depacketizer::h264 → Annex B
                                        │
                                        ▼
                          davey::decrypt(user_id, VIDEO, frame)
                                        │
                                        ▼
                     video_pipeline::decode_video_frame(peer_uid, annex_b)
                                        │
                                        ▼
                     get_active_stream_frames() / UI callback (RGB frame)
```

### 4.3 Data flow (send camera)

```
cameras crate → BGRA frame → VideoEncoder (H264 Annex B)
   → davey::encrypt(VIDEO, H264, frame)
   → packetizer::h264 (FU-A, 1200-byte payloads, marker)
   → transport_crypto::encrypt (per packet + 4-byte nonce)
   → parent UDP socket (SSRC = Ready stream ssrc / rtx for retransmits)
```

Go Live is identical but writes to the **stream** UDP socket and uses the stream DAVE session.

### 4.4 Concurrency and ownership

- Keep one Tokio task per voice connection for the WebSocket.
- Keep one task for the UDP receive loop (already exists); it must dispatch to audio and video pipelines.
- Keep the audio path real-time and lock-free-ish; video decoding stays on the existing decode worker.
- `ssrc_map` and `NativeVideoSession` live in `Arc<Mutex<…>>`/`Arc<RwLock<…>>` like the current stores.
- Never block the Slint event loop: reuse the latest-frame store + `invoke_from_event_loop` pattern.
- DAVE sessions are **not** `Send`-friendly for concurrent encrypt/decrypt? `DaveSession` is used behind a mutex
  today; keep that. For video + audio on the same parent connection, both must serialize through the same session mutex.

### 4.5 Config/feature flags

Add to `.litecord_video_settings.json` (via `src/utils/video_settings.rs`):

```json
{
  "native_discord_video": true,
  "native_video_codec": "h264",
  "camera_device": 0,
  "camera_resolution": "1280x720",
  "camera_fps": 30,
  "go_live_max_bitrate_kbps": 4000
}
```

Keep `LTPV` enabled by default for backward compatibility with other Litecord users; native Discord video is additive.

---

## 5. Implementation plan

Each phase must leave the tree compiling and audio unaffected. Do not attempt all phases in one patch.

### Phase 0 — Prerequisites (after merge conflicts are resolved)

- [ ] Finish the merge; `cargo check` and `cargo test` on the host.
- [ ] Add a `native_video` feature gate (or config flag) so unfinished code cannot break calls.
- [ ] Extract the current AES-GCM audio decrypt into `native_video::transport_crypto` **without changing behavior**
      (unit-test old vs new output on captured packets).
- [ ] Add RTP/RTCP parsing + tests with synthetic packets. No wiring yet.

**Acceptance:** audio calls still work; `cargo test` green; parser handles CSRC/extension/marker.

### Phase 1 — Protocol truth: v9, Identify, codecs, video state

- [ ] Connect with `?v=9`; include `channel_id` in Identify.
- [ ] Send `video: true` and one stream descriptor (`rid:"100"`, `quality:100`).
- [ ] Parse Ready `streams[]`; store local `video_ssrc`/`rtx_ssrc`.
- [ ] Add `codecs[]` to Select Protocol: Opus/120, H264/103 + RTX/104, `encode:true`, `decode:true`.
- [ ] Read and log `audio_codec`, `video_codec`, `media_session_id`, `dave_protocol_version` from Session Description.
- [ ] Send one op 12 with `active:false` on connect (safe no-op), then `active:true` when camera starts.
- [ ] Handle inbound op 12 correctly (split from op 11); cache `user_id`, `audio_ssrc`, `video_ssrc`,
      `rtx_ssrc`, `streams[]`.
- [ ] Handle op 13 (remove SSRCs), op 11 (join).

**Acceptance:** joining a voice channel still carries audio; logs show Ready video SSRCs and inbound op 12 from a
Discord client with camera on; no `Invalid SSRC` disconnect.

### Phase 2 — Receive camera

- [ ] Split the UDP loop: RTCP vs RTP vs control (IP discovery/ping).
- [ ] Route video SSRCs; ignore unknown SSRCs briefly (queue 500 ms) instead of treating them as audio.
- [ ] Add jitter buffer (reorder window ~200 ms), loss detection, NACK generation.
- [ ] Add H264 depacketizer (single NAL, STAP-A, FU-A), start-code normalization.
- [ ] DAVE-decrypt with `MediaType::VIDEO` **after** depacketization.
- [ ] Feed `decode_video_frame()`; request keyframe (PLI) on decode failure / missing IDR.
- [ ] Send op 15 wants (100 for the active view, 0 when closed) with `pixelCounts` on resize.
- [ ] UI: camera should appear automatically like Discord (no Watch button), with the existing popout.
- [ ] On op 12 `video_ssrc: 0`, clear the stream and show the avatar.

**Acceptance:** in a DM call and a guild call, a friend's Discord camera renders live and stops cleanly.
CPU/memory within the existing budget.

### Phase 3 — Transmit camera

- [ ] Reuse the existing camera capture and H.264 encoder (Annex B).
- [ ] Send op 12 active with the Ready SSRCs before the first frame.
- [ ] DAVE-encrypt the complete frame, packetize (FU-A), transport-encrypt, send on the parent UDP socket.
- [ ] 90 kHz timestamps, marker on the last packet, RTCP SR every 5 s.
- [ ] Handle inbound NACK (retransmit via RTX) and PLI/FIR (force keyframe).
- [ ] Respect server op 15 wants: lower bitrate/fps/resolution instead of ignoring them.
- [ ] UI toggle "Camera" with device/resolution/fps; stop sends op 12 `video_ssrc: 0`, empty `streams`.

**Acceptance:** an official Discord client sees Litecord's camera in a DM and guild call, with audio unaffected.

### Phase 4 — Go Live receive

- [ ] Main gateway commands: op 20 Watch, op 19 Delete, op 21 Ping.
- [ ] Parse STREAM_CREATE / STREAM_SERVER_UPDATE / STREAM_UPDATE / STREAM_DELETE.
- [ ] `stream_conn.rs`: second WS + UDP + IP discovery + Select Protocol + Session Description + DAVE.
- [ ] DAVE group id from the stream's `media_session_id` (verify against `rtc_server_id - 1`).
- [ ] Op 12 on the stream connection, op 15 wants, receive/decode with the same pipeline as Phase 2.
- [ ] Build the correct key: `guild:{gid}:{cid}:{owner}` / `call:{cid}:{owner}`; fix the DM path.
- [ ] UI: "Watch" on `self_stream` only (not `self_video`); "Stop watching" sends op 19.
- [ ] On STREAM_DELETE or endpoint null, close only the stream connection.

**Acceptance:** a friend's Discord Go Live in a guild renders in Litecord; leaving the voice channel cleans up.

### Phase 5 — Go Live transmit

- [ ] Main gateway op 18 Create Stream (`guild` or `call`), handle STREAM_CREATE + STREAM_SERVER_UPDATE.
- [ ] Connect the stream WS, Identify with `rtc_server_id`/`rtc_channel_id`, video streams.
- [ ] Own DAVE session and transport key; send op 12, H264 video RTP, RTCP.
- [ ] Screen audio as Opus with `SOUNDSHARE` speaking flag.
- [ ] Pause/resume (op 22), stop (op 19); reconnect on STREAM_SERVER_UPDATE with a new endpoint/token.
- [ ] UI "Go Live" with source picker, resolution/fps/bitrate; keep the existing browser share working.

**Acceptance:** Discord users can watch Litecord's Go Live (video + system audio) in a guild and a DM call.

### Phase 6 — macOS and polish

- [ ] ScreenCaptureKit capture backend + AVFoundation camera + CoreAudio loopback (separate track).
- [ ] Simulcast layers (`rid 50/100`) only if bandwidth evidence demands it; start single-layer.
- [ ] Statistics panel: codec, resolution, fps, bitrate, packet loss, RTT, DAVE state.
- [ ] FEC for native? Not part of Discord's UDP video; rely on NACK/RTX/PLI.
- [ ] Optional H265/AV1 decode later; H264 is the compatibility baseline.

---

## 6. Code-level integration map

| File | Change |
|---|---|
| `src/gateway/voice.rs` | `?v=9`; Identify (`channel_id`, `video:true`, streams); Select Protocol codecs; Session Description fields; split op 11/12/13; RTCP demux; call `native_video::on_rtp/on_rtcp`; shared transport decrypt; send op 5/12/15; keep audio path intact |
| `src/gateway/client.rs` | Add `GatewayCommand::{CreateStream, WatchStream, DeleteStream, SetStreamPaused, PingStream}`; write op 18/19/20/21/22; dispatch STREAM_* events into `GatewayEvent`; fix watch trigger (Go Live only) and key format; keep `session_id` for stream Identify |
| `src/gateway/types.rs` | `GatewayEvent::{StreamCreated, StreamServerUpdated, StreamUpdated, StreamDeleted, NativeVideoStateChanged, NativeVideoFrame?}`; stream structs |
| `src/main.rs` | Start `native_video` on voice connect; wire frames into the existing latest-frame store / participant model; distinguish camera vs Go Live; stream card actions |
| `src/screen_capture/video_pipeline.rs` | Export a small `decode_annex_b(peer_uid, bytes)` wrapper (or reuse `decode_video_frame`); ensure SPS/PPS cache is used for native streams |
| `src/screen_capture/types.rs` | Native stream state map alongside `LTPV`; `set_peer_streaming` must not lie; add `native_video_active(uid)` |
| `src/screen_capture/manager.rs` | Keep `LTPV`; expose encoder factory/camera capture to `native_video` (avoid duplicate capture) |
| `src/utils/video_settings.rs` | New config keys (native video, camera, bitrate) |
| `Cargo.toml` | Possibly add `h264-reader`/`rtp` crates? Prefer hand-rolled parse (small, testable); keep deps minimal |
| `ui/appwindow.slint` | Camera auto-view, Go Live watch/unwatch, quality indicator; reuse popout |

### 6.1 Suggested public API of `native_video`

```rust
pub struct NativeVideoConfig {
    pub enabled: bool,
    pub codec: VideoCodec,             // H264 only for now
    pub send_h264_payload_type: u8,    // 103
    pub send_rtx_payload_type: u8,     // 104
    pub opus_payload_type: u8,         // 120
}

pub struct NativeVideoSession { /* ssrc map, jitter, dave handle, decoder bridge */ }

impl NativeVideoSession {
    pub fn new(cfg: NativeVideoConfig, my_user_id: u64, group_id: u64) -> Self;

    // Gateway events
    pub fn on_ready(&mut self, ready: &VoiceReady);
    pub fn on_session_description(&mut self, sd: &SessionDescription);
    pub fn on_video_state(&mut self, v: &VideoState);      // op 12
    pub fn on_client_disconnect(&mut self, user_id: u64);  // op 13
    pub fn on_media_sink_wants(&mut self, wants: &SinkWants); // op 15

    // Media
    pub fn on_rtp(&mut self, packet: &[u8]);   // after transport decrypt
    pub fn on_rtcp(&mut self, packet: &[u8]);
    pub fn set_watch(&mut self, user_id: u64, want: bool); // emits op 15

    // Encode/send hook
    pub fn send_frame(&mut self, annex_b: &[u8], ts_90k: u32, keyframe: bool) -> Vec<Vec<u8>>;

    // UI
    pub fn take_frames(&mut self) -> Vec<(u64, DecodedFrame)>;
}
```

Keep the gateway-specific JSON shaping in `gateway/`, and keep `native_video` transport-only. This makes it testable
without a live Discord connection.

---

## 7. Test plan

### 7.1 Unit tests (no network)

- **RTP parser:** version 2, padding, CSRC count, extension bit, one-byte extensions, marker, PT mask, truncated packets.
- **Transport crypto:** AES-GCM and XChaCha20 with known key/nonce/frame; verify AAD = header(+CSRC+ext preamble);
  verify 4-byte suffix stripping; tamper tests must fail; nonce replay must be rejected.
- **RTCP:** parse SR/RR/NACK/PLI; build NACK and PLI; round-trip.
- **Jitter buffer:** reorder 3 packets; duplicate; 10% loss produces NACKs; late packet after playout is dropped.
- **H264 depacketizer:** single NAL, STAP-A (multiple NALs), FU-A start/middle/end, out-of-order FU-A, missing end,
  start-code conversion 3↔4 bytes, SPS/PPS extraction.
- **H264 packetizer:** 1-byte frame, 1200-byte boundary, 100 KB keyframe, marker only on last packet, FU-A headers.
- **SSRC map:** op 12 updates, rtx mapping, disconnect cleanup, unknown SSRC queue/drop.
- **DAVE:** with `davey` test vectors or a loopback group, encrypt→depacketize→decrypt yields the original Annex B frame;
  unencrypted passthrough works.
- **Stream key:** guild vs call formatting, DM `guild_id: null`.

### 7.2 Integration tests (live Discord, manual)

Use a normal Discord account logged in on the official client on a second machine/phone. Do not automate self-bots.

| Scenario | Steps | Pass criteria |
|---|---|---|
| Camera receive, DM | Friend starts camera in DM; Litecord joins | Video appears < 1 s, audio unaffected, stop clears |
| Camera receive, guild | Friend starts camera in voice channel | Same; multiple cameras switch correctly |
| Camera transmit, DM | Litecord starts camera | Official client shows video; friend hears audio; stop hides video |
| Camera transmit, guild | Same in a guild | Same; no `Invalid SSRC` |
| Go Live receive | Friend starts Go Live | Watch works, screen audio plays, quality adapts |
| Go Live transmit | Litecord starts Go Live | Official client can watch; system audio is audible |
| Network loss | `tc netem loss 5%` / Wi-Fi drop | NACK/RTX recovers; PLI requests keyframe; no permanent freeze |
| Reconnect | Kill Wi-Fi 5 s | Voice reconnects (resume); stream tears down/re-subscribes |
| DAVE transition | Join/leave another member mid-call | Video continues after epoch transition |
| Popout | Open/close popout, resize | Frames continue, wants update, no leak |
| Resource | 1080p60 for 10 min | Within existing CPU/RAM budget; no unbounded queues |

### 7.3 Diagnostics to add

- Log one line per state change: `op 12 user=… audio=… video=… rtx=… rid=… active=…`.
- Counters: RTP received/lost/reordered, NACK sent/received, PLI sent/received, DAVE pass/fail, decode fail,
  frames dropped at queue, current bitrate/fps, RTCP RTT.
- A `--capture-rtp <ssrc> <file>` debug flag writing decrypted RTP for offline analysis.
- Never log keys or decrypted media.

### 7.4 Known verification gaps

- Exact payload type used by remote senders after negotiation (we advertise 103/104; the SFU may preserve each
  sender's PT). Mitigation: classify by SSRC map + payload sniffing (H264 start code) and log PTs during Phase 1.
- Stream DAVE group id (`media_session_id` vs `rtc_server_id - 1`). Mitigation: log both and try in that order.
- Whether a pure viewer must offer local `streams[]` in stream Identify. Docs suggest `video: true` is enough;
  verify with a capture.
- Simulcast: official clients may publish multiple RIDs; start by requesting only `100` and `any:50`.

---

## 8. Risks and constraints

- **Protocol drift:** Discord changes voice internals without notice. Isolate parsing, keep logs, make failures
  non-fatal to audio.
- **DAVE enforcement:** if a call requires DAVE and our video frames are unencrypted, they are dropped. Do not ship
  video transmit before `MediaType::VIDEO` encrypt works.
- **Invalid SSRC disconnects:** sending video before op 12, or with an SSRC not assigned in Ready, disconnects the
  whole voice session (audio too). Gate transmit behind a strict state machine.
- **Merge conflicts:** the tree is mid-merge. Never implement on top of `UU` files; finish the merge or use a clean worktree.
- **macOS:** screen/camera/audio backends do not exist yet; do not promise macOS in the same delivery as protocol work.
- **Terms of service:** Litecord is a third-party client. Testing with automated user accounts (self-bots) violates
  Discord's ToS; test with the user's own account and a second official client.
- **Security:** native video keys come from Discord (transport) and DAVE (E2EE). Do not log them; wipe stream state on
  disconnect; never reuse the parent connection's DAVE session for a stream connection.

---

## 9. Resume instructions ("haz esto")

When the user asks to continue:

1. **Verify the tree is clean enough:**
   ```bash
   cd /home/kes/Litecord
   git status --short | grep '^UU' && echo "STILL CONFLICTED" || echo "OK"
   cargo check
   ```
   If conflicts remain, stop and ask the user to finish the merge (or create a clean worktree from `HEAD`).
2. **Re-read this document**, especially §3 (gaps) and §5 (phases). Do not restart research.
3. **Create a branch:** `feat/native-discord-video` (or a worktree) and implement **Phase 1 only** first.
4. **Before writing code**, re-check these symbols because line numbers drift:
   - `src/gateway/voice.rs::connect_voice_gateway` (URL, Identify, Select Protocol, op 4, UDP loop, op 11/12).
   - `src/gateway/client.rs` VOICE_STATE_UPDATE / VOICE_SERVER_UPDATE handlers and the WS write path.
   - `src/gateway/types.rs::GatewayEvent`.
   - `src/main.rs` `start_receiver` and the stream frame callbacks.
   - `src/screen_capture/video_pipeline.rs::decode_video_frame`.
5. **Do not delete `LTPV`.** It stays as the Litecord↔Litecord path. Native Discord video is additive.
6. **Ship in phases with acceptance evidence** (logs/screenshots) per §5. Audio must never regress.
7. **Update this document** at the end of each phase: mark checkboxes, record payload-type observations, and note
   any protocol deviation found.

### 9.1 Definition of done for the whole request

- [ ] Litecord sees a Discord friend's camera in a DM call.
- [ ] Litecord sees a Discord friend's camera in a guild voice channel.
- [ ] Litecord sees a Discord friend's Go Live (video + stream audio).
- [ ] Official Discord clients see Litecord's camera in DM and guild.
- [ ] Official Discord clients see Litecord's Go Live (desktop) in DM and guild.
- [ ] Desktop share works on Windows and Linux/Wayland (X11 too).
- [ ] macOS capture is either implemented or explicitly reported as pending.
- [ ] Audio calls, Litecord↔Litecord streaming, and browser sharing still work.
- [ ] Tests from §7 pass; no key/material logging.

---

## 10. References

- Discord official voice docs: `https://discord.com/developers/docs/topics/voice-connections`
- Discord gateway events (streams): `https://discord.com/developers/docs/events/gateway-events`
- DAVE whitepaper: `https://daveprotocol.com/` (protocol v1.1)
- `discord-userdoccers` voice reference (local copy):
  `/tmp/opencode/discord-docs/voice-connections.mdx`
- `discord-userdoccers` gateway events (local copy):
  `/tmp/opencode/discord-docs/gateway-events.mdx`
- `davey 0.1.4` source: `~/.cargo/registry/src/*/davey-0.1.4/`
  (`session.rs`, `cryptor/decryptor.rs`, `cryptor/mod.rs` — `Codec::H264`, `MediaType::VIDEO`)
- Existing Litecord docs: `docs/WATCH_ON_DEMAND.md`, `docs/WEB_SHARING.md`, `docs/NETWORK_AND_PROXY.md`,
  `Architecture.md`, `docs/P2P_VIDEO_STREAMING_PLAN.md`
- RFC 3550 (RTP/RTCP), RFC 6184 (H264 RTP payload), RFC 3711 (SRTP header sizing), RFC 5104 (PLI/FIR),
  RFC 4585 (NACK)

---

## Appendix A — Payload cheat sheet

### A.1 Voice gateway opcodes (client ↔ voice server)

| Op | Dir | Name | Notes |
|---|---|---|---|
| 0 | → | Identify | v9: include `channel_id`, `video`, `streams` |
| 1 | → | Select Protocol | `protocol:"udp"`, `data{address,port,mode}`, `codecs[]` |
| 2 | ← | Ready | `ssrc`, `ip`, `port`, `modes`, `streams[]` |
| 3 | ↔ | Heartbeat | `{t, seq_ack}` on v8+ |
| 4 | ← | Session Description | `mode`, `secret_key`, `video_codec`, `media_session_id`, `dave_protocol_version` |
| 5 | ↔ | Speaking | flags: 1 voice, 2 soundshare, 4 priority |
| 6 | ← | Heartbeat ACK | |
| 8 | ← | Hello | `heartbeat_interval` |
| 11 | ← | Clients Connect | `user_ids[]` |
| 12 | ↔ | **Video** | local/remote video state and SSRC map |
| 13 | ← | Client Disconnect | `user_id` |
| 15 | ↔ | **Media Sink Wants** | `{ "<ssrc>": 0..100, "any": 0..100, "pixelCounts": {...} }` |
| 21/22 | ↔ | DAVE prepare/execute transition | already implemented for audio |
| 23 | → | DAVE ready for transition | |
| 24 | ← | DAVE prepare epoch | |
| 25–29 | ↔ | DAVE MLS binary messages | already implemented for audio |

### A.2 Main gateway opcodes used for streams

| Op | Name | Payload |
|---|---|---|
| 18 | Create Stream | `{type, guild_id?, channel_id, preferred_region?}` |
| 19 | Delete Stream | `{stream_key}` |
| 20 | Watch Stream | `{stream_key}` |
| 21 | Ping Stream Server | `{stream_key}` |
| 22 | Set Stream Paused | `{stream_key, paused}` |

Dispatch events: `STREAM_CREATE`, `STREAM_SERVER_UPDATE`, `STREAM_UPDATE`, `STREAM_DELETE`.

### A.3 Constants to add

```rust
pub const VOICE_GATEWAY_VERSION: u8 = 9;
pub const OPUS_PAYLOAD_TYPE: u8 = 120;
pub const H264_PAYLOAD_TYPE: u8 = 103;
pub const H264_RTX_PAYLOAD_TYPE: u8 = 104;
pub const VIDEO_CLOCK_HZ: u32 = 90_000;
pub const RTP_MAX_PAYLOAD: usize = 1200;      // conservative, matches LTPV MTU
pub const JITTER_WINDOW_MS: u64 = 200;
pub const RTCP_SR_INTERVAL: Duration = Duration::from_secs(5);
```

### A.4 H264 depacketization rules (RFC 6184)

- **Single NAL:** payload type byte `(b & 0x1F)` in `1..=23` → the payload is one NAL unit.
- **STAP-A (24):** 1-byte header, then repeated `[u16 size][NAL]`; emit each NAL with a start code.
- **FU-A (28):** 2-byte header. `S` bit starts a fragmented NAL; `E` ends it; the middle bytes append.
  Reconstruct the NAL header from `FU indicator (F|NRI)` + `FU header type`.
- Ignore **FU-B (29)** unless a sender is known to use it.
- Annex B output: prepend `00 00 00 01` per NAL; keep SPS/PPS before IDR.
- Marker bit on the last RTP packet of the access unit tells the decoder a frame is complete.

### A.5 RTCP feedback

- **Generic NACK** (RTPFB, FMT 1): PID + BLP bitmask; ask for missing sequence numbers.
  Retransmit as RTX: RTX payload = `[u16 original_seq][original payload]`, sent on `rtx_ssrc` with RTX PT.
- **PLI** (PSFB, FMT 1): request a keyframe. Send on decode failure, view open, or after a long gap.
- **FIR** (PSFB, FMT 4): stronger keyframe request; rare.
- Send **SR** every ~5 s for each sent SSRC; consume **RR** for loss/jitter/RTT stats.

---

## Appendix B — Example gap-fix checklist for `voice.rs`

This is a sketch of the Phase 1 diff, not final code:

```rust
// 1. URL
format!("wss://{}/?v=9", clean_endpoint)

// 2. Identify
"d": {
    "server_id": guild_id,          // "@me" for DM parent connection
    "channel_id": channel_id,
    "user_id": user_id,
    "session_id": session_id,
    "token": token,
    "video": true,
    "streams": [{ "type": "video", "rid": "100", "quality": 100 }],
    "max_dave_protocol_version": 1
}

// 3. Select Protocol
"codecs": [
  { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120 },
  { "name": "H264", "type": "video", "priority": 1000,
    "payload_type": 103, "rtx_payload_type": 104,
    "encode": true, "decode": true }
]

// 4. Session Description
let video_codec = val["d"]["video_codec"].as_str().unwrap_or("H264");
let media_session_id = val["d"]["media_session_id"].as_str().unwrap_or("");
let dave_version = val["d"]["dave_protocol_version"].as_u64().unwrap_or(0);

// 5. opcodes
11 => { /* user_ids */ }
12 => { /* video state: cache ssrcs, do NOT treat as join */ }
13 => { /* remove user */ }

// 6. UDP demux (first byte / PT)
match classify_packet(&decrypted) {
    Packet::Rtcp => native_video.on_rtcp(&decrypted),
    Packet::Rtp { payload_type } if payload_type == OPUS_PAYLOAD_TYPE => audio_path(...),
    Packet::Rtp { .. } => native_video.on_rtp(&decrypted),
}
```

---

*End of document. Update the checklists and observations after each implementation phase; do not create a second
competing plan.*
