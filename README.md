# RESET EXPLODE para Linux

App para Linux que mostra e cuida do hardware do PC: sistema, sensores, placa de vídeo, discos, energia, rede,
jogos (biblioteca, FPS estimado, gargalo, overlay), benchmarks, stress test, overclock, iluminação RGB,
otimização, inicialização, apps, drivers e portas seriais. Núcleo em Rust, interface em Tauri 2.

É o port para Linux do [RESET EXPLODE](https://github.com/GustavoReset/ResetExplode) de Gustavo (GustavoReset),
originalmente um app WPF/.NET para Windows, sob a licença MIT (veja `LICENSE`). Os comentários "portado do
`X.cs`" no código indicam de qual arquivo do app original cada parte veio.

## Estrutura

```
crates/reset-core/          tudo que lê ou mexe no hardware, sem interface (testável)
  src/system.rs             SO, placa-mãe/BIOS, CPU, memória        (/proc, /sys/class/dmi)
  src/sensors.rs            sensores ao vivo com mín./máx.           (/sys/class/hwmon, /proc/stat, NVML)
  src/gpu.rs                ficha e sensores da GPU NVIDIA           (NVML, carregado em tempo de execução)
  src/disks.rs, devices.rs  discos e unidades montadas; USB e quais têm LEDs
  src/energy.rs, network.rs energia (bateria, perfis, CPU, GPU) e rede (adaptadores, diagnóstico de lag)
  src/games/                biblioteca (Steam, Heroic, Lutris), FPS estimado, HowManyFPS, gargalo
  src/overlay.rs, oc.rs     MangoHud e overclock (NVML)
  src/bench/, stress.rs     benchmarks de CPU, memória e disco; stress test
  src/lighting/             efeitos RGB + cliente do OpenRGB SDK
  src/tweaks.rs, startup.rs otimização e inicialização
  src/packages.rs, apps.rs  atualizações e drivers (pacman via pkexec); apps instalados e catálogo
  src/maker/                portas seriais, monitor serial e drivers seriais
  src/bin/reset-probe.rs    mostra no terminal o que o app enxerga
crates/reset-gpu/           benchmarks e stress de GPU em Vulkan (programa à parte, janela própria)
app/
  src-tauri/                janela Tauri 2: os comandos que a interface chama (areas/ e jobs.rs)
  ui/                       HTML/CSS/JS puro (módulos ES, sem bundler nem npm)
```

## Instalar

Baixe o instalador na página de [releases](https://github.com/mathauscm/ResetExplodeLinux/releases/latest):

| Sistema | Arquivo | Como |
|---|---|---|
| Qualquer distro (Arch, CachyOS, Manjaro…) | `reset-explode_<versão>_x86_64.AppImage` | marque como executável (`chmod +x`, ou Propriedades → Executável) e abra |
| Ubuntu, Debian, Mint, Pop!_OS | `reset-explode_<versão>_amd64.deb` | dois cliques, ou `sudo apt install ./reset-explode_<versão>_amd64.deb` |
| Fedora | `reset-explode-<versão>-1.x86_64.rpm` | dois cliques, ou `sudo dnf install ./reset-explode-<versão>-1.x86_64.rpm` |

## Desenvolver

Dependências (Arch/CachyOS):

```sh
sudo pacman -S --needed webkit2gtk-4.1 base-devel shaderc openrgb
```

```sh
cargo run -p reset-explode                  # abre o app
cargo run -p reset-core --bin reset-probe   # system | gpu | sensors | disks | usb | updates | drivers | apps | catalog | rgb
cargo test -p reset-core
```

Para instalar no seu usuário (aparece no menu de aplicativos, sem sudo): `./install-local.sh`.
Para atualizar, rode de novo; para remover, `./install-local.sh --remove`.

Para gerar os instaladores de uma release (AppImage, .deb e .rpm, já com o `reset-gpu` dentro, mais o
`SHA256SUMS`) em `dist/`: `cargo install tauri-cli --version '^2'` uma vez e depois `./scripts/release.sh`. A versão
vem de `app/src-tauri/tauri.conf.json`.

## Iluminação RGB

O app conversa com o servidor SDK do OpenRGB (porta 6742), como o backend OpenRGB do app original.
O OpenRGB controla a placa-mãe MSI (Mystic Light), periféricos Razer via USB, memórias RGB e muitos outros.
Efeitos: cor fixa, respiração, arco-íris, temperatura da CPU, temperatura da GPU, carga da CPU e apagar, no PC
inteiro ou com cor própria por peça. "Parar e devolver" volta cada dispositivo ao modo e às cores de antes; ao
fechar o app as cores ficam, e a opção "Aplicar minhas cores ao ligar o PC" as reaplica no login.

- Ao abrir a tela de iluminação o app conecta sozinho; se o OpenRGB estiver fechado, ele inicia
  `openrgb --server` (só o servidor, sem janela) e espera a detecção dos dispositivos.
- Memórias e algumas GPUs precisam de `i2c-dev` (`sudo modprobe i2c-dev`).
- Não use OpenRGB e OpenRazer controlando o mesmo periférico Razer ao mesmo tempo.

## Situação

Todas as telas do app original têm a versão Linux:

| Grupo | Telas |
|---|---|
| RESET EXPLODE | Visão geral, Sistema, Sensores, Placa de vídeo, Discos, Energia, Rede |
| RESET JOGOS | Biblioteca (Steam, Heroic, Lutris), FPS estimado, Gargalo, Overlay (MangoHud) |
| RESET DESEMPENHO | Benchmark (render, simulação, memória, disco, cena e ray tracing na GPU), Stress test, Overclock |
| RESET FERRAMENTAS | Iluminação RGB (OpenRGB), Otimização, Inicialização, Apps, Drivers e atualizações |
| RESET MAKER | Portas seriais (com monitor serial), Drivers seriais |

O que muda em relação ao app original (Windows): o FPS dos jogos vem do MangoHud (não dá para ler de fora como o ETW faz);
as notas de CPU não são comparáveis 1:1 com as do Windows (a imagem e a conta são idênticas, mas o código em
Rust é mais rápido que o .NET no mesmo PC); as notas de GPU vêm de cenas equivalentes em Vulkan.

Ainda não tem: sensores da placa-mãe MSI (o chip NCT6687 precisa do módulo `nct6687d`, no AUR), GPUs AMD/Intel
na ficha técnica e detalhes por pente de memória (exigem root).

### Teste de fumaça

`RESET_SMOKE=1 target/debug/reset-explode` abre o app de verdade, passa por todas as telas (5 s cada), imprime
quanto conteúdo cada uma mostrou e os erros de JavaScript que aparecerem, e fecha.

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
