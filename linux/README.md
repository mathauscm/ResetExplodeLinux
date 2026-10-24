# RESET EXPLODE para Linux (Rust + Tauri)

Port do RESET EXPLODE (WPF/.NET, Windows) para Linux. O app do Windows continua na raiz do repositório.
Aqui ficam o núcleo em Rust e a interface em Tauri.

## Estrutura

```
linux/
  crates/reset-core/        tudo que lê ou mexe no hardware, sem interface
    src/system.rs           SO, placa-mãe/BIOS, CPU, memória       (/proc, /sys/class/dmi)
    src/sensors.rs          sensores ao vivo com mín./máx.          (/sys/class/hwmon, /proc/stat, NVML)
    src/gpu.rs              ficha e sensores da GPU NVIDIA          (NVML, carregado em tempo de execução)
    src/disks.rs            discos e unidades montadas              (/sys/block, /proc/self/mounts)
    src/devices.rs          USB conectado e quais têm LEDs
    src/lighting/           efeitos RGB + cliente do OpenRGB SDK (portado do LightBackends.cs)
    src/packages.rs         atualizações e drivers (checkupdates, pacman via pkexec)
    src/apps.rs             apps instalados (atalhos .desktop + pacman) e catálogo
    src/bin/reset-probe.rs  mostra no terminal o que o app enxerga
  app/
    src-tauri/              janela Tauri 2: só os comandos que a interface chama
    ui/                     HTML/CSS/JS puro (módulos ES, sem bundler nem npm)
```

## Rodar

Dependências (Arch/CachyOS):

```sh
sudo pacman -S --needed webkit2gtk-4.1 base-devel openrgb
```

```sh
cd linux
cargo run -p reset-explode                  # abre o app
cargo run -p reset-core --bin reset-probe   # system | gpu | sensors | disks | usb | rgb [host:porta]
cargo test -p reset-core
```

Para instalar no seu usuário (aparece no menu de aplicativos, sem sudo): `./install-local.sh`.
Para atualizar, rode de novo; para remover, `./install-local.sh --remove`.

Para gerar AppImage/deb/rpm: `cargo install tauri-cli --version '^2'` e depois `cargo tauri build` em `app/src-tauri`.

## Iluminação RGB

O app conversa com o servidor SDK do OpenRGB (porta 6742), igual ao backend OpenRGB do app do Windows.
O OpenRGB controla a placa-mãe MSI (Mystic Light), periféricos Razer via USB, memórias RGB e muitos outros.
Efeitos: cor fixa, respiração, arco-íris, temperatura da CPU, temperatura da GPU, carga da CPU e apagar.
Ao parar (ou fechar o app), cada dispositivo volta ao modo que tinha antes.

- Ao abrir a tela de iluminação o app conecta sozinho; se o OpenRGB estiver fechado, ele inicia
  `openrgb --server` (só o servidor, sem janela) e espera a detecção dos dispositivos.
- Memórias e algumas GPUs precisam de `i2c-dev` (`sudo modprobe i2c-dev`).
- Não use OpenRGB e OpenRazer controlando o mesmo periférico Razer ao mesmo tempo.

## Situação

| Tela | Estado |
|---|---|
| Visão geral, Sistema, Sensores, Placa de vídeo, Discos | feito |
| Iluminação RGB (OpenRGB) | feito e testado no hardware (placa MSI + memória Kingston Fury) |
| Drivers e atualizações (pacman + pkexec) | feito |
| Apps: instalados, remover, catálogo para instalar | feito |
| Energia, Rede | a fazer |
| Biblioteca, FPS estimado, Gargalo, Overlay (MangoHud) | a fazer |
| Benchmark (CPU: port direto; GPU: Vulkan), Stress, Overclock | a fazer |
| Otimização, Inicialização | a fazer (equivalentes Linux) |
| Portas seriais, Drivers seriais | a fazer (udev, grupo `uucp`) |

Ainda não tem: sensores da placa-mãe MSI (o chip NCT6687 precisa do módulo `nct6687d`, no AUR), GPUs AMD/Intel
e detalhes por pente de memória (exigem root).
