# Phase 2 — Screen sharing on macOS (ScreenCaptureKit + CoreAudio)

> Target: full screen-share feature parity for the **sender** on macOS — monitor and window capture at 1080p60, in-game/desktop audio in the stream, CPU (OpenH264) encoding. The **viewer** side needs zero new code (the `not(windows)` CPU-decode fallback already exists).
> Estimated effort: **~1–2 weeks** — this is the bulk of the port.

---

## 0. What is reused vs. what is new

| Pipeline stage | Windows | macOS (this phase) |
| :--- | :--- | :--- |
| Capture | WGC / DXGI Desktop Duplication / GDI (`sources/windows.rs`, `gpu_capture.rs`) | **NEW** `sources/macos.rs` — ScreenCaptureKit `SCStream` |
| Frame format | GPU texture → NV12 (zero-copy) | CVPixelBuffer → BGRA (CPU) into the existing `capture_screen_rgb`-shaped frame path |
| Desktop audio | WASAPI process loopback (`audio/wasapi_loopback.rs`) | **NEW** CoreAudio output tap (macOS arm in `audio_loopback.rs`) |
| Encode | NVENC / AMF / WMF (GPU) or OpenH264 (CPU) | **REUSE** OpenH264 — `create_best_encoder()` already routes non-Windows to it (`src/encoder/mod.rs:122-245`) |
| Packetize / FEC / crypto / signaling | LTPV 1350-byte UDP + XOR FEC + AES-256-GCM + CF Worker | **REUSE** — platform-neutral (`screen_capture/{crypto,signaling,net_diagnostics,manager}`) |
| View | DXVA GPU decode + DirectComposition layer | **REUSE** CPU decode fallback (`hw_decode/mod.rs:66-70` → `HwError::Unsupported` → `video_pipeline`) |

So the phase is really **two new OS modules + wiring + permissions UX**, not a protocol change.

---

## Task 1 — `src/screen_capture/sources/macos.rs` (the core of the phase)

### 1.1 Crate choice

ScreenCaptureKit (macOS 12.3+) is the only supported capture API (ScreenCaptureKit replaced the deprecated `CGDisplayStream`). Options:

- **`screen-capture-kit`** (high-level Rust wrapper): fastest to a working prototype; check its `objc2`/`dispatch` dependency versions against what `cameras 0.1` already pulls in (`objc2 0.6`, `objc2-core-video`, `objc2-av-foundation`, `dispatch2 0.3`) to avoid duplicate-incompatible objc2 versions in the graph.
- **Raw `objc2` + `objc2-core-video` + `objc2-quartz`**: full control, guaranteed version alignment with `cameras`, more code.

Decision heuristic: prototype with `screen-capture-kit` first; if its API can't express what we need (per-window `SCContentFilter`, `SCStreamConfiguration` sample-rate/pixel-format control, `SCStreamAdapter` delivery), drop to raw `objc2`. Either way the public surface stays the three-function seam below, so the choice is contained.

### 1.2 The seam — three functions, same shape as `windows.rs` / `linux_portal.rs`

`src/screen_capture/sources/mod.rs:12-19` re-exports three names per platform. Today:

```rust
#[cfg(windows)]  pub use windows::{capture_screen_rgb, list_capturable_windows, list_screens};
#[cfg(not(windows))] pub use linux_portal::{capture_screen_rgb, list_capturable_windows, list_screens};
```

Change to a three-way split:

```rust
#[cfg(windows)]    pub use windows::{...};
#[cfg(target_os = "macos")] pub use macos::{...};
#[cfg(target_os = "linux")] pub use linux_portal::{...};   // narrow the fallback to Linux
```

and implement in `macos.rs`:

| Function | macOS implementation |
| :--- | :--- |
| `list_screens()` | `SCShareableContent.current()` → `displays` (name, width, height, scale). This call **triggers the TCC Screen Recording prompt** on first use — see Task 5 for the denied/undecided UX. |
| `list_capturable_windows()` | `SCShareableContent.current()` → `windows` (title, owning app, `SCContentFilter`). SCK exposes no window icons (unlike Windows `WM_GETICON` in `windows.rs:193`) — see Open Questions. |
| `capture_screen_rgb(out_bgra, ...)` | Not a per-call grab on macOS: `SCStream` pushes frames continuously. Implementation: ensure an `SCStream` for the selected display/window filter is running (lazy start on first call, matching how the Linux portal pushes into `PORTAL_FRAME`), copy the latest CVPixelBuffer into `out_bgra`, return the frame origin/size. The existing `#[cfg(not(windows))]` worker in `manager.rs:651-779` already calls this in a loop and feeds the shared encode path — no manager changes needed beyond what the seam provides. |

### 1.3 Frame format

`SCStreamConfiguration`: request `kCVPixelFormatType_32BGRA` (the pipeline's native format; `capture_screen_rgb` consumers expect BGRA) at the display's native scale, `width/height` = capture bounds, `minimumFrameInterval = 1/60`. If SCK delivers 10-bit or another format for a given display, convert via `CVPixelBuffer` → `CVPixelBufferPool`/`vImage` or a manual path — verify what 1080p60 costs on Apple Silicon before optimizing; the Windows GDI fallback path proves the CPU pipeline tolerates plain BGRA copies.

### 1.4 Lifecycle

Mirror the existing stop/start contract in `manager.rs` (`stop()` at L168-190 already has a `#[cfg(not(windows))]` arm that clears portal state — extend/add a `#[cfg(target_os = "macos")]` arm that tears down the `SCStream` and its adapter thread). `SCStream` must be stopped before the app quits or the process lingers (known SCK gotcha: the stream's internal processes outlive a sloppy exit).

---

## Task 2 — Desktop audio for streams: CoreAudio output tap

`src/screen_capture/audio_loopback.rs::start_audio_loopback_tx` has per-OS blocks: Windows (WASAPI isolated process loopback, L536-681) and Linux (pactl null-sink + gst pulsesrc, L453-534). **macOS falls through both and the thread idles** (L703+).

Add a `#[cfg(target_os = "macos")]` block that:

1. Finds Litecord's own **output** device (the cpal output stream's device — the same device voice/stream audio plays through).
2. Starts a **CoreAudio tap on that output** (`kAudioUnitSubType_MusicEffect`/output-tap AU, or `AudioObjectAddPropertyListener` + `AudioBufferList` read of the output stream's `kAudioHardwareProperty...` tap) delivering 48 kHz mono/f32 or i16 into the **existing shared PCM buffer** — everything downstream (resample → Opus → AES-GCM → UDP, L695+) is already OS-agnostic.
3. Registers a `Drop`-style cleanup symmetric with the Windows handle / Linux `gst_child` cleanup (L786-796).

Semantics match: like the WASAPI *process* loopback and the Linux null-sink trick, we capture **Litecord's own output** (the stream sender's audio as heard), not the whole system — so other apps' sound stays out of the stream.

**Permissions:** an output tap requires the **Microphone** TCC grant (macOS classifies all audio capture that way) → `NSMicrophoneUsageDescription` (already in Phase 1 Change 4) and a first-use prompt.

**Settings list:** `src/ui/helpers.rs:460-480` currently offers `auto / pulsesrc / cpal` on non-Windows. Add a macOS arm: `auto / coreaudio_tap / cpal`, and route `auto` → tap (fall back to cpal input devices if the tap can't start, mirroring the Windows fallback at L542-681).

---

## Task 3 — Encoder & settings lists

- **v1 encoder = OpenH264 (CPU).** `create_best_encoder()` (`src/encoder/mod.rs:122-245`) already sends non-Windows to the settings string → OpenH264. No encoder code changes.
- **Trim the settings list** in `src/ui/helpers.rs:366-392` (non-Windows currently shows `auto / nvenc / "ffmpeg (VA-API / QSV)" / openh264`): on macOS show `auto / openh264` only — the nvenc/ffmpeg probes are Linux-shaped (the dlopen probe fails harmlessly, but offering them is misleading). Add a `#[cfg(target_os = "macos")]` arm.
- **Capture backend list** `helpers.rs:394-414` (non-Windows: `auto / portal / x11`): macOS arm → `auto / screencapturekit` (or just `auto` — there is exactly one backend; a single-option dropdown is honest).
- **Browser DRM launcher** (`main.rs:1203-1241`, Windows-only): optional macOS arm launching the default browser with a clean profile — defer to Phase 3 unless trivial.

---

## Task 4 — UI conditionals

`ui/appwindow.slint:167` has `in-out property <bool> is_linux`, set only under `#[cfg(target_os = "linux")]` (`main.rs:148-151`). It gates: the screens-tab label (`:3898`), hiding the window-capture tab (`:3928`), portal source display (`:4077/:4086`), window-list visibility (`:4105`).

macOS today evaluates as "not Linux" → it **shows** the window-capture tab (correct for SCK!) but with Linux-shaped labels/options (fixed by Task 3). Two options:

- **Minimal (recommended for v1):** leave `is_linux` as-is; fix only the option *lists* (Task 3). Accept the "portal/x11"-free lists coming from the new macOS arms.
- **Cleaner:** add an `is_macos` property (set under `#[cfg(target_os = "macos")]`) and route the label conditionals at `:3898`/`:4077` through it. Small Slint diff; do it if the labels look wrong in testing.

---

## Task 5 — Permissions & first-run UX

macOS gates the feature behind TCC; the app must handle all three states gracefully:

| State | Detection | UX |
| :--- | :--- | :--- |
| Undecided | `SCShareableContent.current()` triggers the system prompt | Let the prompt show on first "Share screen"; if the user declines, show an in-app hint: *System Settings → Privacy & Security → Screen Recording → enable Litecord, then restart* |
| Denied | Shareable content list comes back **empty** (or `CGPreflightScreenCaptureAccess()` == false where usable) | Persistent banner in the stream settings with a "Open System Settings" button (`x-applesystemsettings:` URL) — do **not** silently show a dark rectangle |
| Granted | Non-empty display/window list | Normal flow. Note: a grant made while the app runs requires an **app restart** to take effect — say so in the banner |

Microphone (audio tap) and Camera (camera source) prompt on first use via the Info.plist strings; a denied mic should degrade to "video-only stream" with a notice, not a failure.

---

## Test plan

1. **Unit-ish:** the portable bins already in `src/bin/` are the harness — `test_lan_stream`, `test_p2p_wan`, `test_stun_wan`, `test_fec`, `test_video_benchmark` compile and run on macOS unchanged.
2. **Local E2E:** two Litecord instances (or one + the portable test client) on the same network: start a monitor stream → viewer sees 1080p60; start a window stream; verify in-stream audio (play a known tone/video through Litecord's output, confirm it arrives at the viewer); stop/cleanup leaves no orphan `SCStream` processes (`pgrep` check).
3. **Permission matrix:** fresh user (all prompts), denied screen recording (banner + settings deep-link), denied mic (video-only degradation), grant-then-restart flow.
4. **Performance:** measure CPU% of a 1080p60 OpenH264 encode on Apple Silicon (expect: meaningfully above the Windows GPU number — this is the honest v1 trade-off; record it in the README benchmarks table as macOS-specific). Watch the DeepSleep/tray behavior: the `SCStream` adapter thread must not keep the app awake at render rate while hidden (the whole point of the project) — cap/stop the stream when the window is hidden, matching `manager.rs` stop semantics.
5. **Intel + Apple Silicon:** run the matrix on both (CI can at least `cargo check` both arches).

---

## Open questions (resolve during implementation)

1. **`screen-capture-kit` vs raw `objc2`** — decide after a half-day spike; the seam (Task 1.2) keeps the rest of the phase independent of the choice.
2. **Window icons** in the capturable-windows list: SCK doesn't expose them. Options: skip (check the UI tolerates missing icons — `windows.rs:193` feeds `extract_window_icon_rgba`), or join with `CGWindowListCopyWindowInfo` + the owning app's `NSWorkspace` icon.
3. **10-bit / HDR displays:** what pixel format does SCK actually deliver on an HDR external display, and does the BGRA conversion path handle it, or do we clamp the config to 8-bit SDR?
4. **Multiple displays / Spaces:** verify `SCContentFilter` behavior capturing a specific display while the user switches Spaces; the Windows path has equivalent quirks (WGC vs DXGI fallback) — the `auto` backend logic in `manager.rs:361-646` has a macOS-shaped decision to make (there's no second backend to fall back to; "auto" = SCK, period).
5. **Minimum macOS version:** SCK needs 12.3. State the floor in the README release notes (and in the `.app` `LSMinimumSystemVersion`, Phase 3).
6. **Stream audio while the app is in the tray:** the tap captures Litecord's output — confirm behavior when the app is suspended in DeepSleep (should stop cleanly with the stream, per Task 1.4 lifecycle).
