# Phase 1 — Make it compile & run on macOS

> Target: `cargo check --bin litecord` and a full `cargo build` succeed on macOS, and the app runs with **login, chat, DMs, voice, and tray all functional**. Screen share / loopback / hotkeys remain inert (documented stubs) — Phases 2–3 replace them.
> Estimated effort: **~1 day** (including first-time macOS dependency compile times).

---

## Change 1 — Add `libc` to the macOS target (THE hard blocker)

**Root cause.** `src/encoder/nvenc.rs` has a `#[cfg(not(target_os = "windows"))]` branch (≈ L213-270 in `try_new()`, ≈ L602-620 in `Drop`) that probes for FFmpeg via `libc::dlopen/dlsym/dlclose`. But `libc` is declared only in:

```toml
# Cargo.toml:121-125
[target.'cfg(target_os = "linux")'.dependencies]
gtk = "0.18"
x11rb = { version = "0.14", features = ["allow-unsafe-code"] }
ashpd = { version = "0.13", default-features = false, features = ["async-io", "screencast"] }
libc = "0.2"
```

On a macOS target `libc` is not in the dependency graph → **E0433: failed to resolve: use of undeclared crate or module `libc`**.

**Fix.** Add a macOS target section next to the existing ones:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
libc = "0.2"
```

(Phase 2 will add `screen-capture-kit` or `objc2*` crates to this same section.)

**Harmless side effect to be aware of:** on macOS the non-Windows probe dlopens `libavcodec.so.6x` / `libavutil.so.6x` — Linux `.so` names that don't exist on macOS. The probe simply fails and `create_best_encoder()` (`src/encoder/mod.rs:122-245`) falls through to **OpenH264**, which is exactly the encoder macOS v1 uses. NVENC does not exist on macOS, so no behavior is lost. (Optionally tidy the probe to `.dylib` names under a `#[cfg(target_os = "macos")]` — not required.)

**Why not widen the cfg gate instead?** Changing `#[cfg(not(target_os = "windows"))]` → `any(target_os = "linux", target_os = "macos")` in `nvenc.rs` also works, but the codebase convention is *target dependency sections + generic `not(...)` fallbacks* (see `docs/MACOS_PORT_PLAN.md` §2). Adding the dep keeps the diff minimal and consistent.

---

## Change 2 — Gate the four ungated Windows test binaries

A full `cargo build` compiles **every** `[[bin]]`; four of them use Windows APIs with no cfg gate and will not compile on macOS:

| Binary | File | Problem |
| :--- | :--- | :--- |
| `test_wmf` | `src/bin/test_wmf.rs:3-8` | Top-level `use windows::{core, Direct3D, Direct3D11, Dxgi, MediaFoundation, Com}` — `windows` isn't a macOS dep |
| `test_nvenc` | `src/bin/test_nvenc.rs:267+` | `fn main()` calls `windows_sys::...LoadLibraryA/GetProcAddress` (dxgi, d3d11, nvcuda), ungated |
| `test_ffmpeg_nvenc` | `src/bin/test_ffmpeg_nvenc.rs:15+` | `fn main()` uses `windows_sys` `SetDllDirectoryA/LoadLibraryA/GetProcAddress` (OBS/scrcpy DLL hunting), ungated |
| `test_amd_sender` | `src/bin/test_amd_sender.rs:37+` | `fn main()` uses `windows_sys` DLL loading, ungated |

**Fix — follow the pattern already in the repo.** `src/bin/test_capture_profiler.rs` does exactly this:

```rust
#[cfg(windows)]
fn main() { /* ...real Windows body... */ }

#[cfg(not(windows))]
fn main() {
    eprintln!("test_capture_profiler is Windows-only");
}
```

Apply the same shape to the four files above (wrap the real `main` and its Windows-only helpers in `#[cfg(windows)]`, add a stub `fn main` under `#[cfg(not(windows))]`). `test_amd_amf_sender.rs` shows the variant where every item is gated.

**Note:** CI runs `cargo check --bin litecord`, which does **not** compile the other bins — so Change 1 alone makes CI green. Change 2 is needed for local `cargo build` (all bins) and for anyone building the test harness on a Mac.

Bins that are already fine on macOS (no action): `test_amd_amf_sender`, `test_capture_profiler`, `test_gpu_encoder`, `test_dxgi` (pure OpenH264 despite the name), `audio_test`, `decode_raw_opus`, `test_amd_amf_pipeline`, `test_audio_pipeline`, `test_ecdh_p2p`, `test_fec`, `test_lan_stream`, `test_p2p_wan`, `test_stream_loopback`, `test_stun_wan`, `test_video_benchmark`. The portable ones double as the Phase 2 test harness.

---

## Change 3 — Open links with `open`, not `xdg-open`

`src/main.rs:505-514` (`on_open_link`): the `#[cfg(not(windows))]` branch shells out to `xdg-open`, which does not exist on macOS → links in chat silently fail.

**Fix.** Add a macOS arm (keep the existing windows/`xdg-open` arms):

```rust
#[cfg(target_os = "macos")]
{
    let _ = std::process::Command::new("open").arg(&url).spawn();
}
```

(Verify the exact shape of the existing closure when implementing — the http/https whitelist validation upstream of it is shared and stays.)

---

## Change 4 — Bundle basics: Info.plist usage strings

Required the moment we run a **bundled** `.app` (dev convenience now; mandatory before distribution in Phase 3):

| Key | Why |
| :--- | :--- |
| `NSMicrophoneUsageDescription` | cpal microphone capture (voice). A bundled app missing it **crashes** when macOS shows the prompt |
| `NSCameraUsageDescription` | `cameras` (AVFoundation) camera capture in streams |
| `CFBundleIdentifier` / `CFBundleName` / `CFBundleExecutable` | Standard bundle identity; also what TCC keys grants to |

Screen Recording needs **no** Info.plist key — the system prompt is triggered by `SCShareableContent.current()` in Phase 2.

For bare `cargo run` development (no bundle) macOS still prompts and records the grant against the binary's path/signature; usage strings are not enforced. So this change is only needed for the `.app` wrapper — a minimal hand-rolled bundle (or `cargo-mobile2`/manual `Info.plist` + `codesign -s -`) suffices; Phase 3 formalizes it.

---

## Verification checklist

```bash
# 1. The check CI will run
cargo check --bin litecord

# 2. Full build (all bins) — validates Change 2
cargo build --release

# 3. Run it
open target/release/Litecord.app      # if bundled
# or: cargo run                        # bare binary; expect TCC prompts for mic/camera

# 4. Functional smoke test
#    - QR login + password login work
#    - Chat / DMs render, send, receive
#    - Voice: join a channel, talk, hear others (CoreAudio via cpal)
#    - Tray icon works; minimize-to-tray suspends rendering
#    - Links in chat open in the default browser (Change 3)

# 5. Confirm the documented inert behavior (Phase 2/3 scope, must NOT crash):
#    - Start a stream → dark rectangle preview, no panic
#    - Global hotkeys → no-op (no listener thread)
#    - HUD → 0%/0MB (no /proc on macOS)
```

**Definition of done for Phase 1:** all of the above passes on both an Apple Silicon Mac and (ideally) an Intel Mac; `ci.yml`'s new `check-macos` job is green (see Phase 3 for the job definition — it can land with this phase since it only needs Change 1).
