# 🎙️ Voice Engine & Audio Pipeline

Litecord features an ultra-low latency, professional-grade voice communications engine implemented in `src/gateway/voice.rs`, `src/audio/`, and `src/screen_capture/audio_loopback.rs`.

---

## 1. Subsystem Architecture

```text
[ Microphone Hardware ] ──► [ CPAL WASAPI Capture Stream ]
                                     │
                                     ▼
                     [ Voice Activity Detection (VAD) ]
                                     │
                                     ▼
                     [ Opus Encoder (48kHz Stereo, 64kbps) ]
                                     │
                                     ▼
                     [ DAVE E2EE Frame Encryption (AES-GCM) ]
                                     │
                                     ▼
                     [ UDP Socket Transport (RTP) ]
                                     │
                                     ▼ (Network)
                     [ Jitter Buffer & Loss Concealment ]
                                     │
                                     ▼
                     [ DAVE E2EE Decryption ]
                                     │
                                     ▼
                     [ Opus Decoder (48kHz Stereo PCM) ]
                                     │
                                     ▼
                     [ Tactical IGL Ducking & Volume Matrix ]
                                     │
                                     ▼
[ Speakers / Headphones ] ◄── [ CPAL WASAPI Playback Stream ]
```

---

## 2. Dynamic Output Device Switching (Zero-Disconnection Hot-Swap)

Previously, switching output devices required restarting the app or re-joining the voice room. Litecord implements **Dynamic Audio Device Switching**:
- The active audio playback stream is wrapped in an atomic handle (`Arc<Mutex<Option<cpal::Stream>>>`).
- When the user selects a new output device in Settings, the voice loop receives an event without tearing down the Voice Gateway connection.
- The playback stream is cleanly rebuilt and bound to the new audio endpoint with zero audio glitching and zero packet drops.

---

## 3. Tactical IGL & Shot-Caller Speech Priority Ducking

Litecord provides competitive squads with built-in voice ducking:
- Users can be assigned a priority level (`P:1`, `P:2`).
- When a user with Priority 2 speaks (e.g. In-Game Leader calling a strat), the audio pipeline dynamically applies a volume attenuation factor (e.g., -12 dB) to all other participants and bot streams.
- As soon as the shot-caller stops speaking, audio volumes smoothly ramp back to normal, ensuring critical calls are never drowned out.

---

## 4. Voice Activity Detection (VAD) & Real-Time VU Meter

- Real-time Root Mean Square (RMS) calculation:
  $$RMS = \sqrt{\frac{1}{N}\sum_{i=1}^{N} x_i^2}$$
- Normalized to a linear 0.0 to 1.0 sensitivity scale.
- Threshold slider allows users to tune their mic cut-off so mechanical keyboards, breathing, and PC fans remain silent while normal speech triggers immediate voice activation.

---

## 5. Discord DAVE E2EE Protocol

Litecord implements the Discord DAVE (Discord Audio/Video End-to-End Encryption) protocol:
- Uses Messaging Layer Security (MLS) principles to negotiate ratchet keys across voice call participants.
- Audio payloads are encrypted at the client level before reaching Discord voice servers.
