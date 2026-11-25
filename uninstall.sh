#!/usr/bin/env bash
# =============================================================================
# Litecord — One-Line Linux Uninstaller
# Usage: curl -sSL https://raw.githubusercontent.com/Ak4ai/Litecord/main/uninstall.sh | bash
#   Keep settings and saved accounts:  ... | bash -s -- --keep-data
#   Remove them without asking:        ... | bash -s -- --purge
# =============================================================================
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/256x256/apps"

RESET='\033[0m'; BOLD='\033[1m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[0;33m'
info()    { echo -e "${CYAN}${BOLD}[litecord]${RESET} $*"; }
success() { echo -e "${GREEN}${BOLD}[litecord]${RESET} $*"; }
warn()    { echo -e "${YELLOW}${BOLD}[litecord]${RESET} $*"; }

MODE="ask"
for arg in "$@"; do
  case "$arg" in
    --keep-data) MODE="keep" ;;
    --purge)     MODE="purge" ;;
  esac
done

remove() {
  for path in "$@"; do
    if [ -e "$path" ] || [ -L "$path" ]; then
      rm -rf -- "$path"
      success "Removed ${path}"
    fi
  done
}

# Stop a running instance so the binary is not in use.
if command -v pkill >/dev/null 2>&1 && pkill -x litecord 2>/dev/null; then
  info "Closed the running Litecord."
  sleep 1
fi

info "Removing the app..."
remove "${INSTALL_DIR}/litecord" \
       "${DESKTOP_DIR}/litecord.desktop" \
       "${ICON_DIR}/litecord.png" \
       "${HOME}/.local/share/icons/litecord.png" \
       "${HOME}/.config/autostart/litecord.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

# Settings, saved accounts and caches.
DATA=("${HOME}/.config/litecord" "${HOME}/.cache/litecord")
for f in "${HOME}"/.litecord_*; do
  [ -e "$f" ] && DATA+=("$f")
done

if [ "$MODE" = "ask" ]; then
  if [ -r /dev/tty ]; then
    printf '%b' "${CYAN}${BOLD}[litecord]${RESET} Also delete settings and saved accounts? [y/N] "
    read -r answer < /dev/tty || answer=""
    case "$answer" in [yY]*) MODE="purge" ;; *) MODE="keep" ;; esac
  else
    MODE="keep"
  fi
fi

if [ "$MODE" = "purge" ]; then
  remove "${DATA[@]}"
else
  warn "Settings and saved accounts were kept. Run with --purge to delete them."
fi

success "Litecord was uninstalled."
