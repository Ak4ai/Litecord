# 📺 Screen Sharing & Video Pipeline

Litecord incorporates an advanced 1080p 60 FPS video streaming engine built in `src/screen_capture/` and `src/encoder/`.

---

## 1. Capture Pipeline

```text
[ Windows Compositor / DWM ]
            │
            ▼ (DXGI 1.2 OutputDuplication API)
[ Direct3D 11 Zero-Copy Surface ]
            │
            ├─► [ NVIDIA NVENC Hardware Encoder ] ──┐
            ├─► [ AMD AMF VCE Hardware Encoder ]  ──┤
            ├─► [ Windows Media Foundation (WMF) ] ─┼─► [ RTP / LTPV Packetizer ]
            └─► [ Cisco OpenH264 SIMD Software ]  ──┘            │
                                                                 ▼
                                                  [ UDP Stream + XOR FEC Parity ]
```

---

## 2. Hardware Encoders

1. **NVIDIA NVENC (`src/encoder/nvenc.rs`)**:
   - Dedicated silicon NVENC hardware encoder (Turing, Ampere, Ada Lovelace, Blackwell).
   - Zero CPU overhead, sub-5ms encoding latency at 1080p 60 FPS.
2. **AMD AMF (`src/encoder/amf.rs`)**:
   - AMD Advanced Media Framework utilizing Video Coding Engine (VCE) and Video Core Next (VCN) on Radeon hardware.
3. **Windows Media Foundation MFT (`src/encoder/wmf.rs`)**:
   - Native OS hardware H.264 encoder supporting Intel QuickSync, AMD, and NVIDIA.
4. **Cisco OpenH264 (SIMD Software)**:
   - Multithreaded AVX2/SSE4.1 software fallback for systems without supported hardware encoding.

---

## 3. LTPV Protocol & Forward Error Correction (FEC)

- Video frames are split into **1350-byte chunks** (avoiding IP packet fragmentation over standard 1500-byte MTUs).
- Every burst of video packets includes XOR Forward Error Correction (FEC) packets.
- If an intermediate router or Wi-Fi spike drops a packet, the receiver reconstructs the missing packet immediately using the FEC parity slice, eliminating the need for TCP-style retransmissions.

---

## 4. Picture-in-Picture (PiP) Popout Window

- Streams can be popped out into an independent window down to 180px width.
- Supports **Always-on-Top Pinning**.
- Includes a **Click-Through Ghost Mode** (`WS_EX_TRANSPARENT | WS_EX_LAYERED`), allowing gamers to overlay streams on top of games without capturing mouse clicks!
