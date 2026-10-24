#!/usr/bin/env bash
# Gera os instaladores da release em dist/, com a distribuição no nome de cada arquivo:
#   reset-explode-<versão>-qualquer-distro-x86_64.AppImage   (roda em qualquer distro, sem instalar)
#   reset-explode-<versão>-ubuntu-debian-amd64.deb           (Ubuntu, Debian, Mint, Pop!_OS...)
#   reset-explode-<versão>-fedora-x86_64.rpm                 (Fedora)
# cada um já com o reset-gpu dentro, mais o SHA256SUMS e dist/NOTAS.md (o "qual arquivo baixar" da release).
# Precisa do tauri-cli: cargo install tauri-cli --version '^2'. No GitHub, o workflow release.yml roda este script.
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
# só os desta versão: a pasta do Tauri guarda os pacotes de versões anteriores
cp "$B"/appimage/*_"$VERSION"_*.AppImage "dist/reset-explode-$VERSION-qualquer-distro-x86_64.AppImage"
cp "$B"/deb/*_"$VERSION"_*.deb "dist/reset-explode-$VERSION-ubuntu-debian-amd64.deb"
cp "$B"/rpm/*-"$VERSION"-*.rpm "dist/reset-explode-$VERSION-fedora-x86_64.rpm"
chmod +x dist/*.AppImage
(cd dist && sha256sum -- *.AppImage *.deb *.rpm > SHA256SUMS)
sed "s/{{VERSION}}/$VERSION/g" .github/release-notes.md > dist/NOTAS.md
ls -lh dist
