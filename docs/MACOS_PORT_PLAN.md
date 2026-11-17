# 🍎 macOS Port — Findings & High-Level Plan

> **Status:** Planning. Working fork: https://github.com/StreamOfRon/Litecord
> **Goal:** Ship Litecord for macOS (Apple Silicon + Intel) with feature parity to the Windows build.

Litecord currently ships **Windows** (full-featured: GPU screen share, WASAPI loopback, DPAPI vault) and **Linux** (AppImage: portal capture, pactl loopback, AES-GCM vault). This document captures the audit of the codebase's platform surface and the phased plan for a macOS build.

Per-phase detail:
- [Phase 1 — Make it compile & run](MACOS_PORT_PHASE1.md)
- [Phase 2 — Screen sharing (ScreenCaptureKit + CoreAudio tap)](MACOS_PORT_PHASE2.md)
- [Phase 3 — Polish (hotkeys, vault, HUD, updater, signing, CI)](MACOS_PORT_PHASE3.md)

---

## 1. What already works on macOS (no changes needed)

The core is cross-platform Rust; only the OS-specific edges are gated:

| Area | Crate / mechanism | macOS status |
| :--- | :--- | :--- |
| UI | Slint 1.9 + winit (`SLINT_BACKEND=winit-software` default, per `src/main.rs:112-114`) | ✅ Works (CPU-rendered UI is a feature, not a gap) |
| Voice I/O | `cpal` (CoreAudio on macOS) | ✅ Works; macOS takes the plain `not(linux)` config path (`src/audio/engine.rs:219-221`) |
| DAVE E2EE voice | `davey` (MLS RFC 9420) | ✅ Platform-neutral |
| Tray | `tray-icon` (native NSStatusBar) | ✅ Works; the `gtk::init()` call is Linux-gated (`src/utils/tray.rs:63-66`) |
| Camera | `cameras` 0.1 | ✅ Has a built-in AVFoundation backend (objc2) — no feature change needed |
| Networking | `reqwest`/`tokio-tungstenite` with `native-tls` | ✅ Uses the Security framework on macOS |
| P2P video protocol | `str0m` (rust-crypto), LTPV UDP + FEC | ✅ Platform-neutral |
| CPU video codec | `openh264` | ✅ Cross-platform (SIMD is x86-only → Apple Silicon uses the C fallback, slightly slower) |
| Discord identity | `src/utils/client_identity.rs:39-43` | ✅ **Already has a `#[cfg(target_os = "macos")]` branch** — the port was anticipated |
| Crypto | `aes-gcm`, `chacha20poly1305`, `x25519-dalek`, `rsa`, `sha2` | ✅ Platform-neutral |

## 2. Why a third OS is cheap here: the two-tier cfg pattern

The codebase uses a consistent pattern in every subsystem:

1. **Cargo target sections** — OS crates live in `[target.'cfg(windows)'.dependencies]` / `[target.'cfg(target_os = "linux")'.dependencies]` (`Cargo.toml:80-125`).
2. **Primary split** — `#[cfg(windows)]` vs `#[cfg(not(windows))]` at every call site.
3. **Second tier inside `not(windows)`** — `#[cfg(target_os = "linux")]` for truly Linux machinery (ashpd portal, pactl, evdev, x11rb), with a **generic stub** in the remaining `not(...)` arm (dark frame, no-op, zeros) so a third OS *compiles for free*.
4. **Whole-module gating** for irreducibly Windows tech — `gpu_tx`/`gpu_capture`, `hw_decode/{device,dxva,layers}`, `encoder/{amd_amf,wmf,nvenc_api,nvenc_d3d11}`, `audio/wasapi_loopback` — all with `not(windows)` fallbacks at the call sites (CPU decode, `gpu_path = false`, OpenH264).

macOS plugs into the `not(windows)` seam everywhere. The port = (a) fix the one compile break, (b) replace the generic stubs with real macOS implementations where the stub is user-visible.

## 3. The one hard compile blocker

| Blocker | Location | Fix |
| :--- | :--- | :--- |
| `libc` is not in the macOS dependency graph | `src/encoder/nvenc.rs:213-270, 602-620` — the `#[cfg(not(target_os = "windows"))]` branch calls `libc::dlopen/dlsym/dlclose`, but `libc` is declared only under `[target.'cfg(target_os = "linux")'.dependencies]` (`Cargo.toml:121-125`) | Add `libc = "0.2"` to a new `[target.'cfg(target_os = "macos")'.dependencies]` section |

That is the **only** break for `cargo check --bin litecord` (what CI runs). A full `cargo build` additionally fails on four ungated Windows test binaries — see [Phase 1](MACOS_PORT_PHASE1.md#change-2).

## 4. Runtime gaps on macOS (compiles, but inert until ported)

| Feature | macOS behavior today | Fixed in |
| :--- | :--- | :--- |
| Screen capture | Solid dark rectangle (`src/screen_capture/sources/linux_portal.rs:429-437` stub); window list empty (`:390-403`) | Phase 2 (ScreenCaptureKit) |
| Stream desktop audio | Loopback thread idles silently (`src/screen_capture/audio_loopback.rs:703+` — no macOS block) | Phase 2 (CoreAudio tap) |
| Global mute/deafen hotkeys | `start_global_listener` has no macOS branch → empty body (`src/utils/keybinds.rs:267+`) | Phase 3 (CGEventTap) |
| Hardware HUD (CPU/RAM/GPU) | `/proc/self/status` reader → zeros (`src/utils/hardware_monitor.rs:366-388`) | Phase 3 (mach/IOKit) |
| Token vault key | AES-256-GCM path works, but key derivation falls back to a **static salt** (no `/etc/machine-id`, `src/auth/vault.rs:168-170`) | Phase 3 (IOPlatformUUID / Keychain) |
| Open links | Falls into the `xdg-open` branch, absent on macOS (`src/main.rs:505-514`) | Phase 1 (`open`) |
| In-app updater | Non-Windows flow downloads the **Linux tarball** (`src/utils/updater.rs:415+`) | Phase 3 (signed `.app` zip flow) |
| UI sound fallback | Pipes WAV to `pw-play`/`paplay`/`aplay` — all absent (`src/audio/sound_effects.rs:354-400`); primary cpal path still works | Phase 3 (`afplay`) |
| GPU video layers / GPU encode / GPU decode | All `not(windows)` no-ops → CPU path by design (`src/ui/gpu_video.rs`, `src/encoder/mod.rs:249`, `src/screen_capture/hw_decode/mod.rs:66-70`) | Phase 3 stretch (VideoToolbox) — CPU path is acceptable for v1 |
| Settings lists | Show Linux-shaped options: "portal/x11" capture backends, "pulsesrc" loopback, "ffmpeg (VA-API / QSV)" encoder (`src/ui/helpers.rs:366-480`) | Phase 2/3 (macOS option lists) |
| Browser launcher for DRM streams | Entire body `#[cfg(windows)]` → inert (`src/main.rs:1203-1241`) | Phase 3 (nice-to-have) |

## 5. Phase overview

| Phase | Scope | Effort | Detail |
| :--- | :--- | :--- | :--- |
| **1. Compile & run** | `libc` dep, gate 4 test bins, `open` for links, Info.plist usage strings. Result: login, chat, DMs, voice, tray all functional | **~1 day** | [MACOS_PORT_PHASE1.md](MACOS_PORT_PHASE1.md) |
| **2. Screen sharing** | `sources/macos.rs` on ScreenCaptureKit (monitor + window capture), CoreAudio output tap for stream audio, OpenH264 CPU encode, settings-list + UI tweaks. Viewer side needs nothing (CPU decode fallback exists) | **~1–2 weeks** — the bulk of the port | [MACOS_PORT_PHASE2.md](MACOS_PORT_PHASE2.md) |
| **3. Polish & distribution** | CGEventTap hotkeys, mach-based hardware monitor, vault key fix (IOPlatformUUID → Keychain), `afplay` fallback, macOS updater flow, VideoToolbox HW encode (stretch), `.app` bundle + Developer ID signing + notarization, CI jobs | Incremental; **signing infra is a prerequisite for public release** | [MACOS_PORT_PHASE3.md](MACOS_PORT_PHASE3.md) |

## 6. macOS-specific gotchas (read before Phase 2)

1. **TCC permissions are keyed to the code signature (CDHash).** Screen Recording, Microphone and Accessibility grants are tied to the signed binary's identity. An ad-hoc-signed build that changes on every rebuild makes macOS **re-prompt on every update** (and a re-prompted Screen Recording grant requires restarting the app). Consequences:
   - Local dev (ad-hoc / `cargo run`): expect repeated prompts — fine for iteration, fine for testing.
   - **Any distributed build needs a stable Apple Developer ID ($99/yr) + notarization**, or screen sharing becomes a permission-prompt nightmare for users. The in-app updater must preserve the same signing identity.
2. **Screen Recording prompt** is triggered by `SCShareableContent.current()` (ScreenCaptureKit, macOS 12.3+). There is no Info.plist usage string for it; the system prompts. If the user denies, the shareable-content list comes back empty → the UI must point the user to System Settings → Privacy & Security → Screen Recording, and the app must be restarted after a grant.
3. **Microphone / Camera** *do* require Info.plist usage strings (`NSMicrophoneUsageDescription`, `NSCameraUsageDescription`) in a bundled `.app` — a bundled app without them **crashes** when the system tries to prompt.
4. **Apple Silicon:** OpenH264's SIMD is x86-only → the C fallback compiles and works; NEON is a possible later optimization. Build both arches (or a universal binary) for release.

## 7. CI / release integration (summary)

- `ci.yml`: add a `check-macos` job — `macos-latest`, `cargo check --bin litecord` (no system deps needed beyond Xcode CLT).
- `build.yml`: add a `build-macos` job — build (x86_64 + arm64), bundle into `Litecord.app`, `codesign` with Developer ID (repo secrets), `xcrun notarytool submit` + `staple`, zip, upload artifact; add the zip to `create-release`'s file list.
- Signing/notarization credentials live in the fork's repo **Settings → Secrets and variables → Actions** (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`/app-store password or key — see Phase 3).

## 8. Audit source

Full file/line-level inventory of every platform gate in `src/` (screen_capture, encoder, audio, auth, utils, gateway, ui, main, bins) was produced during planning; the load-bearing references are cited inline in the phase documents. Re-verify line numbers against the fork's `main` before starting each phase — the upstream repo moves.
