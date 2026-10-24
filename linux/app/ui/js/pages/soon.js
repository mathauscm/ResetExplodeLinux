// Telas ainda não portadas: mostra o que a versão Linux vai fazer no lugar da do Windows.

import { head } from "../lib.js";

const PLAN = {
  energy: ["Bateria, perfis de energia (power-profiles-daemon / tuned) e governor da CPU."],
  net: ["Teste de latência, perda de pacotes e velocidade, igual ao do Windows (o NetworkTest não depende do sistema)."],
  library: ["Jogos da Steam (incluindo Proton), Lutris e Heroic, com as opções de inicialização por jogo."],
  fps: ["FPS esperado por jogo para a sua GPU + CPU (mesmo catálogo e cliente HowManyFPS do Windows)."],
  bottleneck: ["Detector de gargalo ao vivo e por sessão, com os mesmos critérios do BottleneckAnalyzer."],
  overlay: [
    "No Linux o FPS de um jogo só é lido de dentro dele: o app vai configurar o <b>MangoHud</b> com os itens que você escolher.",
    "O atalho global e a janela sempre por cima têm limitações no Wayland/GNOME.",
  ],
  bench: [
    "<b>Render (CPU)</b>: o mesmo path tracer do RESET BENCH, com a mesma nota (é determinístico).",
    "<b>Cena de jogo e ray tracing (GPU)</b>: reescritos em Vulkan; a RTX 4060 tem os núcleos de RT para o teste de ray query.",
  ],
  stress: ["Stress de CPU e RAM com parada por temperatura; stress de GPU em Vulkan."],
  oc: ["Limite de energia e offsets de clock da NVIDIA pelo NVML (precisa de root)."],
  optimize: ["Governor, gamemode, swappiness, TRIM e limpeza de cache, cada ajuste com desfazer."],
  startup: ["Programas que abrem com o sistema: autostart do desktop e serviços do usuário do systemd."],
  ports: ["Portas /dev/ttyUSB* e /dev/ttyACM*, quem está usando cada uma e o grupo <code>uucp</code>/<code>dialout</code>."],
  serial: ["CH340, CP210x e FTDI já vêm no kernel: diagnóstico do módulo carregado e regras udev para Arduino e ESP32."],
};

export function render(el, item) {
  el.innerHTML = head("Em breve", item.label) +
    `<div class="card"><h2>Na versão Linux</h2><ul class="soon-list">${(PLAN[item.id] ?? ["Em planejamento."]).map((l) => `<li>${l}</li>`).join("")}</ul></div>`;
}
