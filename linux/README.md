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

## Como uma tela nova é ligada

Cada tela tem os próprios arquivos e nada mais precisa ser mexido:

| Camada | Arquivo |
|---|---|
| Lógica (sem interface, testável) | `crates/reset-core/src/<módulo>.rs` |
| Comandos que a interface chama | `app/src-tauri/src/areas/<área>.rs` → `dispatch(app, método, args) -> Result<Value, String>` |
| Tela | `app/ui/js/pages/<tela>.js` → `render(el, item)` (devolve uma função de limpeza, se usar timers) |
| Estilo da tela | `app/ui/css/<tela>.css` |

Na interface: `call("<área>", "<método>", {args})` (de `js/lib.js`) devolve o JSON do `dispatch`.
Eventos do backend: `app.emit("<área>-<evento>", payload)` no Rust e `listen("<área>-<evento>", cb)` no JS.
Tarefas longas com saída ao vivo (com ou sem root via `pkexec`): `jobs::spawn` no Rust e `runJob` (`js/pkgjob.js`) no JS.

Áreas: `energy` (Energia), `network` (Rede), `games` (Biblioteca, FPS estimado, Gargalo), `overlay`, `bench`
(Benchmark), `stress` (Stress test), `oc` (Overclock), `tools` (Otimização, Inicialização), `maker` (Portas e
drivers seriais).

## reset-gpu (benchmarks e stress de GPU)

Programa à parte (`crates/reset-gpu`), porque abre a própria janela Vulkan. O app o procura na mesma pasta
do executável (`~/.local/bin` depois do `install-local.sh`, `target/<perfil>` no desenvolvimento) e lê a saída:
**uma linha JSON por evento** no stdout.

```
reset-gpu info
reset-gpu scene --width 1920 --height 1080 --seconds 30 [--fullscreen]
reset-gpu rt    --preset normal|alta|extrema --width 1920 --height 1080 --seconds 30 [--fullscreen]
reset-gpu burn  --width 1920 --height 1080 --msaa 0|4|8 --intensity normal|alta|extrema --seconds 600 --temp-limit 85 [--fullscreen]
```

| Linha | Quando |
|---|---|
| `{"type":"info","adapter":"NVIDIA GeForce RTX 4060","backend":"vulkan","driver":"…","ray_query":true}` | `info`, e no começo dos outros |
| `{"type":"progress","elapsed":12.5,"fps":143.2,"temp_c":61,"power_w":null}` | ~2x por segundo |
| `{"type":"result","test":"scene","avg_fps":680.1,"low1_fps":512.3,"frames":20403,"seconds":30.0,"score":6801,"stopped":"tempo"}` | no fim (`stopped`: `tempo`, `temperatura` ou `usuario`) |
| `{"type":"error","message":"…"}` | erro (código de saída ≠ 0) |

ESC ou fechar a janela encerra (`"stopped":"usuario"`). Nota: `scene` = FPS médio × 10; `rt` = amostras/s ÷ 10.000.

Detalhes da implementação (ash + winit, GLSL compilado pelo `glslc` no `build.rs`, que precisa do pacote `shaderc`):
- ESC, fechar a janela, SIGINT ou SIGTERM encerram e ainda escrevem o `result` (o app cancela com SIGTERM).
- `rt --seconds` é o tempo total: aquecimento de 1/4 (entre 2 e 20 s) + 3 passadas iguais, nota pela mediana.
  O `progress` traz `"phase"` (`aquecendo`, `passada N de 3`).
- `burn` tem `"score": null` (o contrato só dá nota para `scene` e `rt`). MSAA que a placa não aceita desce sozinho.
- Campos extras opcionais no `result`: todos têm `width`, `height`, `max_temp_c`; `rt` tem `preset`, `spp`,
  `bounces`, `passes`, `variation_pct`, `reliable`, `samples_per_sec`, `gigarays`; `burn` tem `msaa`, `intensity`,
  `temp_c`, `temp_limit`. O `info` também traz `api` e `vram`.
- Padrões: 1920x1080; 30 s em `scene`/`rt` e 600 s no `burn`; preset e intensidade `alta`; MSAA 4; limite de 85 °C.

O Overclock aplica os ajustes rodando o próprio app com root: `pkexec reset-explode --nvml-apply --gpu N
--power-mw … --core-offset … --mem-offset … --clock-cap N|none`. Nesse modo ele recusa rodar sem root e valida
tudo de novo contra os limites lidos na hora.
