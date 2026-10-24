// Sistema: SO, placa-mãe/BIOS, processador e memória (o que o app do Windows lia do WMI).

import { invoke, head, kv, bytes, duration } from "../lib.js";

export async function render(el) {
  el.innerHTML = head("RESET EXPLODE", "Sistema") + `<div class="empty">Lendo o sistema…</div>`;
  const s = await invoke("system_info");
  const mhz = (v) => (v ? `${(v / 1000).toFixed(2).replace(".", ",")} GHz` : null);
  // o kernel informa o cache como "512K" / "32768K"
  const cache = (v) => {
    const kb = parseInt(v, 10);
    return Number.isNaN(kb) ? v : kb >= 1024 ? `${kb / 1024} MB` : `${kb} KB`;
  };

  el.innerHTML = head("RESET EXPLODE", "Sistema") + `<div class="grid wide">
    <div class="card"><h2>Sistema operacional</h2>${kv([
      ["Distribuição", s.os.name],
      ["Kernel", s.os.kernel],
      ["Computador", s.os.hostname],
      ["Ambiente", [s.os.desktop, s.os.session && s.os.session.toUpperCase()].filter(Boolean).join(" · ")],
      ["Ligado há", duration(s.os.uptime_s)],
    ])}</div>
    <div class="card"><h2>Placa-mãe e BIOS</h2>${kv([
      ["Fabricante", s.board.vendor],
      ["Modelo", s.board.model],
      ["Produto", s.board.product],
      ["BIOS", [s.board.bios_vendor, s.board.bios_version].filter(Boolean).join(" · ")],
      ["Data da BIOS", s.board.bios_date],
    ])}</div>
    <div class="card"><h2>Processador</h2>${kv([
      ["Modelo", s.cpu.model],
      ["Núcleos / threads", `${s.cpu.cores} / ${s.cpu.threads}`],
      ["Clock base", mhz(s.cpu.base_mhz)],
      ["Clock máximo", mhz(s.cpu.max_mhz)],
      ["Cache L2 (por núcleo)", cache(s.cpu.l2)],
      ["Cache L3", cache(s.cpu.l3)],
      ["Governor", s.cpu.governor],
      ["Instruções", s.cpu.features.join(", ")],
    ])}</div>
    <div class="card"><h2>Memória</h2>${kv([
      ["Total", bytes(s.memory.total)],
      ["Disponível", bytes(s.memory.available)],
      ["Swap", s.memory.swap_total ? `${bytes(s.memory.swap_total - s.memory.swap_free)} usados de ${bytes(s.memory.swap_total)}` : "desligado"],
    ])}<p class="sub" style="margin-top:12px">Detalhes por módulo (fabricante, velocidade, timings) precisam de root para ler o SPD. Isso entra junto com o helper de permissões.</p></div>
  </div>`;
}
