#!/usr/bin/env bash
# Gera os instaladores da release em dist/: AppImage (qualquer distro), .deb (Ubuntu/Debian) e .rpm (Fedora),
# cada um já com o reset-gpu dentro, mais o SHA256SUMS. Precisa do tauri-cli: cargo install tauri-cli --version '^2'.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' app/src-tauri/tauri.conf.json)
TRIPLE=$(rustc -vV | sed -n 's/^host: //p')

# o reset-gpu vai junto como "sidecar": o Tauri espera o nome com o triplo do alvo
cargo build --release -p reset-gpu
mkdir -p app/src-tauri/binaries
cp target/release/reset-gpu "app/src-tauri/binaries/reset-gpu-$TRIPLE"

# NO_STRIP: o strip do linuxdeploy é antigo e falha com binários de distros novas (seções .relr.dyn)
(cd app/src-tauri && NO_STRIP=true cargo tauri build --bundles appimage,deb,rpm --config tauri.release.conf.json)

rm -rf dist && mkdir -p dist
B=target/release/bundle
cp "$B"/appimage/*.AppImage "dist/reset-explode_${VERSION}_x86_64.AppImage"
cp "$B"/deb/*.deb "dist/reset-explode_${VERSION}_amd64.deb"
cp "$B"/rpm/*.rpm "dist/reset-explode-${VERSION}-1.x86_64.rpm"
chmod +x dist/*.AppImage
(cd dist && sha256sum -- * > SHA256SUMS)
ls -lh dist
