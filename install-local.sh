#!/usr/bin/env bash
# Instala o RESET EXPLODE só para o usuário atual (sem sudo): binário em ~/.local/bin,
# ícone e atalho no menu de aplicativos. Rode de novo para atualizar; com --remove, desinstala.
set -euo pipefail

cd "$(dirname "$0")"
BIN="$HOME/.local/bin/reset-explode"
ICON="$HOME/.local/share/icons/hicolor/256x256/apps/reset-explode.png"
DESKTOP="$HOME/.local/share/applications/reset-explode.desktop"

if [[ "${1:-}" == "--remove" ]]; then
  rm -f "$BIN" "$(dirname "$BIN")/reset-gpu" "$ICON" "$DESKTOP"
  update-desktop-database -q "$HOME/.local/share/applications" 2>/dev/null || true
  echo "RESET EXPLODE removido."
  exit 0
fi

cargo build --release -p reset-explode -p reset-gpu
install -Dm755 target/release/reset-explode "$BIN"
# benchmarks e stress de GPU: programa à parte, ao lado do app (ele o procura na mesma pasta)
install -Dm755 target/release/reset-gpu "$(dirname "$BIN")/reset-gpu"
install -Dm644 app/src-tauri/icons/icon.png "$ICON"
mkdir -p "$(dirname "$DESKTOP")"
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=RESET EXPLODE
GenericName=Hardware e sensores
Comment=Sistema, sensores, placa de vídeo, discos e iluminação RGB
Exec=$BIN
Icon=reset-explode
Terminal=false
Categories=System;Monitor;
Keywords=hardware;sensores;gpu;rgb;openrgb;benchmark;
StartupWMClass=reset-explode
EOF
update-desktop-database -q "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "Instalado: $BIN"
echo "Procure por \"RESET EXPLODE\" no menu de aplicativos."
