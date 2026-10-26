# Phase 3 — Polish, packaging & distribution for macOS

> Target: macOS-specific gaps closed (hotkeys, HUD, vault key, updater, sound fallback), a signed + notarized `Litecord.app` in GitHub Releases, and CI coverage. Each item below is independent and shippable on its own; **the signing/notarization infra (Task 6) is the hard prerequisite for public distribution** — start it early since Apple Developer ID approval takes days.
> Estimated effort: incremental — days per task; signing setup is mostly waiting on Apple.

---

## Task 1 — Global mute/deafen hotkeys (CGEventTap)

`src/utils/keybinds.rs::start_global_listener` has Windows (raw-input message sink, L277-484) and Linux (evdev, L486-697) arms; **macOS falls through to an empty body** → global hotkeys silently dead.

- Add a `#[cfg(target_os = "macos")]` arm using a **CGEventTap** (`.cgSessionEventTap`, `keyDown`/`mouseDown` events) or an `NSEvent` global monitor — both require the user to grant **Accessibility** (System Settings → Privacy & Security → Accessibility).
- On first use: preflight with `AXIsProcessTrusted()`; if not trusted, show the in-app hint (same banner pattern as Phase 2's Screen Recording hint) with a deep link (`x-applesystemsettings:`). Do not block the app on the prompt.
- In-app (local) shortcuts already work via winit — the global tap is an enhancement, so a denied Accessibility grant degrades gracefully: document that in the settings UI.
- Keep the existing `vk_*` key-code vocabulary; map macOS `kVK_*`/`CGKeyCode` into it (the Linux arm's `vk_to_evdev_code` table, L751, is the template for a `vk_to_cgkeycode` map).

## Task 2 — Hardware performance HUD (mach/IOKit)

`src/utils/hardware_monitor.rs:366-388` — the `#[cfg(not(windows))]` sampler reads `/proc/self/status` → **zeros on macOS** (no `/proc`).

- Add a `#[cfg(target_os = "macos")]` `PlatformHardwareSampler`:
  - **RAM:** `task_info(mach_task_self_, TASK_VM_INFO, ...)` → `phys_footprint` (matches what the Windows `K32GetProcessMemoryInfo` arm reports).
  - **CPU%:** `host_processor_info(CPU_LOAD_INFO)` deltas, or per-process `task_threadtimes` deltas — mirror the Windows `GetProcessTimes`/`GetSystemTimes` ratio math (L115-364) so the HUD numbers mean the same thing across OSes.
  - **GPU%:** no first-class per-process API; options: `IOAccelerator`/`IOReport` utilization (works for Apple GPU on recent macOS), or leave 0 with a "n/a" label. Low priority — the titlebar `Litecord - x%/ymb` format only carries CPU/RAM anyway.
- The shared consumers (titlebar update, tray tooltip) are already OS-agnostic — only the sampler changes.

## Task 3 — Token vault key (IOPlatformUUID → Keychain)

`src/auth/vault.rs:165-221` — the `not(windows)` AES-256-GCM path works on macOS, but key derivation reads `/etc/machine-id`, which **doesn't exist on macOS** → falls back to the static salt `"litecord_static_mid_fallback"` (L168-170). That defeats the machine-binding the security docs promise.

- **Short fix (do this first):** derive from **`IOPlatformUUID`** (IOKit — the macOS equivalent of `/etc/machine-id`: stable per hardware, per-machine). `vault.rs` already imports nothing heavy; a small `#[cfg(target_os = "macos")]` reader via `IOServiceGetPropertyMatches`/`IORegistryEntryReadCFData` (or the `core-foundation`/`io-kit` crates) slots into `get_linux_vault_key()`. Vault file location (`~/.config/litecord/session.vault`, 0700/0600, `#[cfg(unix)]` at L141-163) already applies to macOS unchanged.
- **Proper fix (follow-up):** store the vault key (or the tokens) in the **Keychain** (`Security` framework, `kSecClassGenericPassword`, `kSecAttrAccessibleAfterFirstUnlock`) — the macOS-native equivalent of DPAPI's user-session binding. Design note: keep the existing AES-GCM file format so tokens stay portable/readable; Keychain holds only the derived key material.
- ⚠️ Changing the key derivation **orphans existing vaults** — irrelevant for a new OS (no macOS users yet), but never change the Linux/Windows derivations.

## Task 4 — In-app updater (macOS asset flow)

`src/utils/updater.rs:256+` — non-Windows currently downloads the **Linux tarball** and installs to `~/.local/bin/litecord` (wrong asset, wrong layout).

- Add a `#[cfg(target_os = "macos")]` flow: download `litecord-macos-<arch>.zip` (the signed+notarized `.app` zip from Phase 3 Task 6) → verify (stapled notarization ticket / `codesign --verify`) → replace the running bundle.
- **Replacing a running `.app` on macOS** needs the standard dance: write the new bundle to a temp location, `pkill`/exit the running instance, move the new bundle over the old, relaunch (`NSWorkspace`/`open`). The Windows arm's `TerminateProcess` + installer-launch pattern (L322-385) is the template.
- **Signing identity must be stable** across versions (same Developer ID) or every update resets the user's TCC grants (Screen Recording/Mic/Accessibility) — see the gotcha in `MACOS_PORT_PLAN.md` §6.1. The updater should detect an identity change and warn before replacing.

## Task 5 — Small shared-path fixes

| Item | Location | Fix |
| :--- | :--- | :--- |
| UI sound fallback | `src/audio/sound_effects.rs:354-400` | `#[cfg(target_os = "macos")]` arm piping the in-memory WAV to `afplay` (the `not(windows)` arm currently tries `pw-play`/`paplay`/`aplay` — all absent on macOS; primary cpal path is unaffected) |
| Browser DRM launcher | `src/main.rs:1203-1241` (Windows-only body) | Optional macOS arm: launch default browser (`open -na "Google Chrome" --args ...` or `open -j` for a fresh profile) — nice-to-have, low risk |
| Memory trim | `src/ui/helpers.rs:902-930` | `trim_process_memory` is a no-op off-Windows/Linux; macOS: `malloc_zone_pressure_relief` or just `mi_collect` (mimalloc is already a base dep) — optional |
| i18n language detection | `src/utils/i18n.rs:510+` | `not(windows)` arm parses `LANG` — works on macOS; optionally consult `AppleLanguages` for better detection. Cosmetic |

## Task 6 — `.app` bundle, code signing, notarization (the distribution gate)

### 6.1 The bundle

- Layout: `Litecord.app/Contents/{MacOS/litecord, Info.plist, Resources/Litecord.icns}`.
- **Info.plist** (extends Phase 1 Change 4): `CFBundleIdentifier` (e.g. `com.litecord.app` — pick once, never change it), `CFBundleName`, `CFBundleShortVersionString` (from Cargo.toml version), `CFBundleExecutable`, `LSMinimumSystemVersion` (12.3 — the SCK floor), `NSMicrophoneUsageDescription`, `NSCameraUsageDescription`, `LSApplicationCategoryType = public.app-category.social-networking`.
- **Icon:** convert `assets/app_icon.ico` → `.icns` (`iconutil`/`sips`); add the source PNG set to `assets/`.
- Build step: a small `scripts/macos/bundle.sh` (copy binary, write Info.plist, `codesign`) — keeps the bundle reproducible outside Xcode.

### 6.2 Signing

- **Local dev:** `codesign --force --deep -s - Litecord.app` (ad-hoc). Works, but TCC grants follow the CDHash → repeated re-prompts across rebuilds. Acceptable for iteration; **never ship ad-hoc**.
- **Distribution:** **Apple Developer ID** ($99/yr program membership — apply early, approval takes days):
  ```bash
  codesign --force --deep --sign "Developer ID Application: <Name> (<TEAMID>)" Litecord.app
  # then notarize + staple:
  ditto -c -k --keepParent Litecord.app Litecord.zip
  xcrun notarytool submit Litecord.zip --team-id "$APPLE_TEAM_ID" \
      --apple-id "$APPLE_ID" --password "$APPLE_APP_SPECIFIC_PW" --wait
  xcrun stapler staple Litecord.zip
  ```
- Arch strategy: build `x86_64-apple-darwin` and `aarch64-apple-darwin`, then either ship two zips or `lipo` a universal binary (recommend **universal** — one artifact, one signature, one TCC identity).

### 6.3 CI (land the `check-macos` job with Phase 1; the release job with this phase)

**`ci.yml`** — add:

```yaml
  check-macos:
    name: Check macOS Build
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: "macos-ci"
      - run: cargo check --bin litecord
```

(No apt deps — Xcode CLT covers everything Phase 1 needs; ScreenCaptureKit/objc2 link against system frameworks.)

**`build.yml`** — add a `build-macos` job mirroring `build-linux`:

1. `cargo build --release --bin litecord` for `x86_64-apple-darwin` and `aarch64-apple-darwin` (or `--target` universal via lipo of both).
2. `scripts/macos/bundle.sh` → `Litecord.app`.
3. Sign with Developer ID (repo secrets: `APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD`/`APPLE_TEAM_ID`, or `APPLE_ID` + app-specific password for notarization) → notarize → staple → zip.
4. Upload artifact `litecord-macos`; add to `create-release`'s `files:` list (L240-244) and to `needs:` (L203).
5. Signing steps should be **skippable** (e.g. `if: env.APPLE_TEAM_ID != ''`) so fork CI still produces an unsigned build for testing — unsigned builds are dev-only.

**Secrets to configure in the fork** (Settings → Secrets and variables → Actions): `APPLE_TEAM_ID`, `APPLE_ID` + `APPLE_APP_SPECIFIC_PASSWORD` (or a notarization key: `APPLE_KEY`/`APPLE_KEY_PASSWORD`/`APPLE_KEY_P12`), `APPLE_CERTIFICATE` + `APPLE_CERTIFICATE_PASSWORD` for the Developer ID cert.

## Task 7 — Stretch: VideoToolbox hardware encode/decode

Only after Phase 2 is solid and the OpenH264 CPU cost on Apple Silicon is measured:

- **Encode:** a `VideoToolbox` `VTCompressionSession` (H.264/HEVC, low-latency real-time mode) implementing the **CPU `VideoEncoder` trait** (`src/encoder/mod.rs:40` — `encode(bgra) -> frames` is the seam; no texture zero-copy like Windows, but the silicon does the compression). Register it in `create_best_encoder()` as the macOS `auto` choice and add it to the settings list.
- **Decode:** a `VTDecoder` impl behind the `GpuDecoder`-shaped fallback (`hw_decode/mod.rs:54-93` currently returns `HwError::Unsupported` off-Windows) — viewers on Mac would then decode on silicon instead of CPU.
- **OpenH264 on arm64:** the bundled C build uses x86 SIMD only; a NEON pass (or upstreaming one) is a separate optimization if CPU encode proves too hot.

## Task 8 — Docs & release notes

- `README.md`: add macOS to the downloads table (`.app` zip + universal note), the prerequisites section, and an **honest benchmarks row** (CPU encode will be higher than the Windows GPU numbers — measure in Phase 2, don't extrapolate).
- `SECURITY.md`: document the macOS vault story (IOPlatformUUID-derived AES-GCM, Keychain follow-up) next to the DPAPI/machine-id rows.
- `CHANGELOG.md`: macOS entry with the **permission primer** (Screen Recording + Microphone + optional Accessibility; where to find them; restart-after-grant; stable signing identity so updates don't re-prompt).
- `docs/README.md` hub: link the three macOS port docs (done in this plan's commit).
