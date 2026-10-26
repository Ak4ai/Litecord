#!/usr/bin/env bash
# Assemble a minimal Litecord.app for local development on macOS.
#
# Phase 1 of the macOS port (docs/MACOS_PORT_PHASE1.md, Change 4): a
# hand-rolled bundle carrying the Info.plist usage strings macOS requires
# to prompt for microphone/camera access — a bundled app missing them
# crashes when the system tries to show the prompt. Ad-hoc signed.
#
# The bare binary (cargo run) works without a bundle; usage strings are
# not enforced there, so this script is only needed for the .app flow.
# Phase 3 extends this script with the icon, LSMinimumSystemVersion,
# Developer ID signing, and notarization.
#
# Usage: scripts/macos/bundle.sh [path-to-binary]   (default: target/release/litecord)
set -euo pipefail

cd "$(dirname "$0")/../.."

BIN="${1:-target/release/litecord}"
[ -x "$BIN" ] || { echo "error: binary not found: $BIN (run: cargo build --release)" >&2; exit 1; }

APP="target/release/litecord.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/litecord"
cp scripts/macos/Info.plist "$APP/Contents/Info.plist"

codesign --force -s - "$APP"

echo "Built $APP (ad-hoc signed)."
echo "Note: TCC grants are keyed to the ad-hoc signature (CDHash) — every rebuild"
echo "changes it, so macOS re-prompts for microphone/camera after each rebuild."
echo "Run it with: open $APP"
