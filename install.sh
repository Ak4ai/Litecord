#!/usr/bin/env bash
# =============================================================================
# Litecord — One-Line Linux Installer
# Usage: curl -sSL https://raw.githubusercontent.com/Ak4ai/Litecord/main/install.sh | bash
# =============================================================================
set -euo pipefail

REPO="Ak4ai/Litecord"
BINARY_NAME="litecord"
ASSET_NAME="litecord-linux-x64.tar.gz"
INSTALL_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"

# ── Colors ───────────────────────────────────────────────────────────────────
RESET='\033[0m'; BOLD='\033[1m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'
YELLOW='\033[0;33m'; RED='\033[0;31m'

info()    { echo -e "${CYAN}${BOLD}[litecord]${RESET} $*"; }
success() { echo -e "${GREEN}${BOLD}[litecord]${RESET} $*"; }
warn()    { echo -e "${YELLOW}${BOLD}[litecord]${RESET} $*"; }
error()   { echo -e "${RED}${BOLD}[litecord] ERROR:${RESET} $*" >&2; exit 1; }

echo -e "${BOLD}"
echo "  ╔═══════════════════════════════════════╗"
echo "  ║       Litecord Linux Installer        ║"
echo "  ║   Ultra-Lightweight Discord Client    ║"
echo "  ╚═══════════════════════════════════════╝"
echo -e "${RESET}"

# ── Check required tools ─────────────────────────────────────────────────────
for cmd in curl tar; do
  command -v "$cmd" >/dev/null 2>&1 || error "Required tool not found: '$cmd'. Please install it first."
done

# ── Check & Install System Dependencies ───────────────────────────────────────
info "Checking runtime dependencies for Linux..."
if command -v pacman >/dev/null 2>&1; then
  info "Arch Linux detected. Installing runtime dependencies (xdotool, libayatana-appindicator)..."
  sudo pacman -S --needed --noconfirm xdotool libayatana-appindicator libappindicator-gtk3 2>/dev/null || true
  if [ ! -f /usr/lib/libxdo.so.3 ] && [ -f /usr/lib/libxdo.so.4 ]; then
    sudo ln -sf /usr/lib/libxdo.so.4 /usr/lib/libxdo.so.3 2>/dev/null || true
  fi
elif command -v apt-get >/dev/null 2>&1; then
  info "Debian/Ubuntu detected. Installing runtime dependencies (libayatana-appindicator3-1, xdotool)..."
  sudo apt-get update -qq 2>/dev/null || true
  sudo apt-get install -y --no-install-recommends libayatana-appindicator3-1 xdotool 2>/dev/null || true
elif command -v dnf >/dev/null 2>&1; then
  info "Fedora detected. Installing runtime dependencies (libayatana-appindicator-gtk3, xdotool)..."
  sudo dnf install -y libayatana-appindicator-gtk3 xdotool 2>/dev/null || true
fi

# ── Fetch latest release ──────────────────────────────────────────────────────
info "Fetching latest release info from GitHub..."
LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"
RELEASE_JSON=$(curl -sSL "$LATEST_URL")

DOWNLOAD_URL=$(echo "$RELEASE_JSON" \
  | grep -o '"browser_download_url": *"[^"]*'"${ASSET_NAME}"'"' \
  | grep -o 'https://[^"]*')

[ -z "$DOWNLOAD_URL" ] && error "Could not find asset '${ASSET_NAME}'. Check: https://github.com/${REPO}/releases"

VERSION=$(echo "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | grep -o '"[^"]*"$' | tr -d '"')
info "Latest version: ${BOLD}${VERSION}${RESET}"

# ── Download & extract ────────────────────────────────────────────────────────
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

info "Downloading ${ASSET_NAME}..."
curl -sSL --progress-bar "$DOWNLOAD_URL" -o "${TMP_DIR}/${ASSET_NAME}"

info "Extracting archive..."
tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "$TMP_DIR"

# ── Install binary ────────────────────────────────────────────────────────────
mkdir -p "$INSTALL_DIR"

BINARY_PATH=$(find "$TMP_DIR" -name "$BINARY_NAME" -type f | head -1)
[ -z "$BINARY_PATH" ] && error "Binary '${BINARY_NAME}' not found in archive."

chmod +x "$BINARY_PATH"
cp "$BINARY_PATH" "${INSTALL_DIR}/${BINARY_NAME}"
success "Binary installed: ${BOLD}${INSTALL_DIR}/${BINARY_NAME}${RESET}"

# ── Install application icon ──────────────────────────────────────────────────
ICON_DIR="${HOME}/.local/share/icons/hicolor/256x256/apps"
mkdir -p "$ICON_DIR"
mkdir -p "${HOME}/.local/share/icons"

ICON_SRC=$(find "$TMP_DIR" -name "app_icon.png" -type f | head -1)
if [ -n "$ICON_SRC" ] && [ -f "$ICON_SRC" ]; then
  cp "$ICON_SRC" "${ICON_DIR}/litecord.png"
  cp "$ICON_SRC" "${HOME}/.local/share/icons/litecord.png"
else
  # Fallback download icon directly from GitHub repository
  curl -sSL "https://raw.githubusercontent.com/${REPO}/main/assets/app_icon.png" -o "${ICON_DIR}/litecord.png" 2>/dev/null || true
  cp "${ICON_DIR}/litecord.png" "${HOME}/.local/share/icons/litecord.png" 2>/dev/null || true
fi
success "Icon installed: ${BOLD}${ICON_DIR}/litecord.png${RESET}"

# ── Create .desktop entry ─────────────────────────────────────────────────────
mkdir -p "$DESKTOP_DIR"
cat > "${DESKTOP_DIR}/litecord.desktop" << EOF
[Desktop Entry]
Name=Litecord
Comment=Ultra-Lightweight Native Discord Client
Exec=${INSTALL_DIR}/${BINARY_NAME}
Icon=litecord
Terminal=false
Type=Application
Categories=Network;InstantMessaging;
StartupWMClass=litecord
EOF
success "Desktop entry created: ${DESKTOP_DIR}/litecord.desktop"

# Refresh desktop database / icon cache if tools available
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

# ── PATH check ────────────────────────────────────────────────────────────────
echo ""
if echo ":${PATH}:" | grep -q ":${INSTALL_DIR}:"; then
  success "All done! Run ${BOLD}litecord${RESET} to start."
else
  warn "${INSTALL_DIR} is not in your PATH yet."
  echo -e "  Add to your ~/.bashrc or ~/.zshrc:\n"
  echo -e "    ${BOLD}export PATH=\"\$HOME/.local/bin:\$PATH\"${RESET}\n"
  echo -e "  Then reload: ${BOLD}source ~/.bashrc${RESET}  (or restart your terminal)\n"
  info "Or run directly: ${BOLD}${INSTALL_DIR}/${BINARY_NAME}${RESET}"
fi

echo ""
success "Litecord ${VERSION} installed successfully! 🚀"
echo ""
