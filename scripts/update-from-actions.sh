#!/usr/bin/env bash
# =============================================================================
# Litecord — instala el binario Linux compilado por GitHub Actions
#
# Descarga el release rodante "latest" del fork, lo valida y reemplaza el
# binario local de forma atómica (guardando una copia en <binario>.old).
#
# Uso:
#   ./scripts/update-from-actions.sh              # actualizar ahora
#   ./scripts/update-from-actions.sh --force      # reinstalar aunque sea igual
#   ./scripts/update-from-actions.sh --install-timer   # actualización diaria
#   ./scripts/update-from-actions.sh --remove-timer    # desactivar timer
#
# Variables opcionales:
#   LITECORD_REPO   (default: stickyfran/Litecord)
#   LITECORD_TAG    (default: latest)
#   LITECORD_ASSET  (default: litecord-linux-x64)
#   LITECORD_BIN    (default: ~/.local/bin/litecord)
# =============================================================================
set -euo pipefail

REPO="${LITECORD_REPO:-stickyfran/Litecord}"
TAG="${LITECORD_TAG:-latest}"
ASSET="${LITECORD_ASSET:-litecord-linux-x64}"
DEST="${LITECORD_BIN:-${HOME}/.local/bin/litecord}"
URL="${LITECORD_URL:-https://github.com/${REPO}/releases/download/${TAG}/${ASSET}}"

RESET='\033[0m'; BOLD='\033[1m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'
YELLOW='\033[0;33m'; RED='\033[0;31m'
info()    { echo -e "${CYAN}${BOLD}[litecord]${RESET} $*"; }
success() { echo -e "${GREEN}${BOLD}[litecord]${RESET} $*"; }
warn()    { echo -e "${YELLOW}${BOLD}[litecord]${RESET} $*"; }
fail()    { echo -e "${RED}${BOLD}[litecord] ERROR:${RESET} $*" >&2; exit 1; }

FORCE=0
ACTION="update"
for arg in "$@"; do
  case "$arg" in
    -f|--force)          FORCE=1 ;;
    --install-timer)     ACTION="install-timer" ;;
    --remove-timer)      ACTION="remove-timer" ;;
    -h|--help)           sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)                   fail "Opción desconocida: $arg (usa --help)" ;;
  esac
done

# ── Timer opcional (systemd de usuario) ──────────────────────────────────────
SELF="$(readlink -f "$0")"
UNIT_DIR="${HOME}/.config/systemd/user"

install_timer() {
  command -v systemctl >/dev/null 2>&1 || fail "systemd no está disponible."
  install -Dm755 "$SELF" "${HOME}/.local/bin/litecord-update"
  mkdir -p "$UNIT_DIR"

  cat > "${UNIT_DIR}/litecord-update.service" <<EOF
[Unit]
Description=Actualiza Litecord desde el build de GitHub Actions
After=network-online.target

[Service]
Type=oneshot
ExecStart=${HOME}/.local/bin/litecord-update --force
Nice=10
EOF

  cat > "${UNIT_DIR}/litecord-update.timer" <<'EOF'
[Unit]
Description=Revisa a diario un nuevo build de Litecord

[Timer]
OnCalendar=daily
RandomizedDelaySec=1h
Persistent=true

[Install]
WantedBy=timers.target
EOF

  systemctl --user daemon-reload
  systemctl --user enable --now litecord-update.timer
  success "Timer diario activado: systemctl --user list-timers litecord-update.timer"
  info "El binario se reemplaza solo; Litecord usará la nueva versión al reiniciarse."
}

remove_timer() {
  systemctl --user disable --now litecord-update.timer 2>/dev/null || true
  rm -f "${UNIT_DIR}/litecord-update.service" "${UNIT_DIR}/litecord-update.timer"
  systemctl --user daemon-reload 2>/dev/null || true
  success "Timer desactivado. (El script ~/.local/bin/litecord-update sigue instalado)."
}

[[ "$ACTION" == "remove-timer" ]] && { remove_timer; exit 0; }
[[ "$ACTION" == "install-timer" ]] && install_timer

# ── Dependencias ─────────────────────────────────────────────────────────────
for cmd in curl install tar; do
  command -v "$cmd" >/dev/null 2>&1 || fail "Falta la herramienta '$cmd'."
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ── Descarga ─────────────────────────────────────────────────────────────────
info "Descargando ${ASSET} desde ${REPO} (${TAG})..."
curl -fL --retry 3 --retry-delay 2 --connect-timeout 15 \
  --progress-bar "$URL" -o "${TMP}/litecord" \
  || fail "No se pudo descargar $URL. ¿Ya terminó el build en Actions?"

[[ -s "${TMP}/litecord" ]] || fail "El archivo descargado está vacío."
# Un HTML de error de GitHub también se descarga con HTTP 200 en algunos casos.
head -c 4 "${TMP}/litecord" | grep -q 'ELF' \
  || fail "El asset no es un binario ELF (¿el release 'latest' aún no existe?)."
[[ "$(stat -c%s "${TMP}/litecord")" -gt 5000000 ]] \
  || fail "El binario es sospechosamente pequeño; descarga abortada."
chmod +x "${TMP}/litecord"

# ── ¿Ya está al día? ─────────────────────────────────────────────────────────
if [[ -x "$DEST" && "$FORCE" -eq 0 ]]; then
  if cmp -s "${TMP}/litecord" "$DEST"; then
    success "Ya tienes el último build instalado (${DEST})."
    exit 0
  fi
fi

if pgrep -x litecord >/dev/null 2>&1; then
  warn "Litecord está en ejecución: el binario se reemplaza igual, pero la versión nueva se usará al reiniciar la app."
fi

# ── Reemplazo atómico con copia de seguridad ─────────────────────────────────
mkdir -p "$(dirname "$DEST")"
if [[ -e "$DEST" ]]; then
  cp -p "$DEST" "${DEST}.old"
  info "Copia de seguridad: ${DEST}.old"
fi

# install escribe primero en un temporal del mismo filesystem y luego hace
# rename, así nunca queda un binario a medias.
install -m755 "${TMP}/litecord" "${DEST}"

success "Binario actualizado: ${DEST}"
info "Ejecuta 'litecord' (o reinicia la app) para usar el build de Actions."
