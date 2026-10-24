// Visão geral: CPU, GPU, memória e discos num relance, atualizando a cada segundo.

import { invoke, snapshot, head, esc, num, fmtPct, fmtMhz, fmtW, bytes, bar, every } from "../lib.js";

export function render(el) {
  el.innerHTML = `${head("Visão geral", "Seu PC agora")}
    <div class="grid">
      <div class="card"><h2>Processador</h2><div id="cpuName" class="sub">—</div><div class="big" id="cpuTemp">—</div><div id="cpuBar"></div>
        <div class="stat-row"><div>Carga<b id="cpuLoad">—</b></div><div>Núcleo + ocupado<b id="cpuTop">—</b></div><div>Clock médio<b id="cpuMhz">—</b></div></div></div>
      <div class="card"><h2>Placa de vídeo</h2><div id="gpuName" class="sub">—</div><div class="big" id="gpuTemp">—</div><div id="gpuBar"></div>
        <div class="stat-row"><div>Carga<b id="gpuLoad">—</b></div><div>Clock<b id="gpuMhz">—</b></div><div>Consumo<b id="gpuW">—</b></div></div></div>
      <div class="card"><h2>Memória</h2><div class="sub" id="ramSub">—</div><div class="big" id="ramPct">—</div><div id="ramBar"></div>
        <div class="stat-row"><div>Em uso<b id="ramUsed">—</b></div><div>VRAM<b id="vram">—</b></div><div>Ativo há<b id="uptime">—</b></div></div></div>
    </div>
    <h2 style="margin-top:22px">Armazenamento</h2>
    <div class="grid" id="vols"></div>`;

  const $ = (id) => el.querySelector("#" + id);

  invoke("system_info").then((s) => {
    $("cpuName").textContent = `${s.cpu.model} · ${s.cpu.cores} núcleos / ${s.cpu.threads} threads`;
    $("ramSub").textContent = `${bytes(s.memory.total)} instalados`;
    $("uptime").textContent = Math.floor(s.os.uptime_s / 3600) + " h";
  });
  invoke("gpu_list").then((g) => ($("gpuName").textContent = g[0] ? `${g[0].name} · ${bytes(g[0].vram_total, 0)}` : "Sem GPU NVIDIA detectada (AMD/Intel em breve)"));
  invoke("disks_list").then((disks) => {
    // partições de boot não interessam num relance; ficam na tela Discos
    const vols = disks.flatMap((d) => d.volumes.map((v) => ({ ...v, disk: d }))).filter((v) => !/^\/(boot|efi)(\/|$)/.test(v.mount));
    const title = (v) => v.label || (v.mount === "/" ? "Sistema" : v.mount.split("/").pop());
    $("vols").innerHTML = vols.length
      ? vols.map((v) => {
          const used = v.total - v.free, pct = v.total ? (100 * used) / v.total : 0;
          return `<div class="card"><div class="row"><b>${esc(title(v))}</b><span class="spacer"></span><span class="tag">${esc(v.disk.kind)}</span></div>
            <div class="sub">${esc(v.mount)} · ${esc(v.disk.model)}</div>
            <div style="margin:10px 0 6px">${bar(pct, pct > 90)}</div>
            <div class="sub">${bytes(v.free)} livres de ${bytes(v.total)}</div></div>`;
        }).join("")
      : `<div class="empty">Nenhuma unidade montada.</div>`;
  });

  return every(1000, async () => {
    const { summary: s } = await snapshot();
    $("cpuTemp").innerHTML = s.cpu_temp_c == null ? "—" : `${num(s.cpu_temp_c)}<small>°C</small>`;
    $("cpuBar").innerHTML = bar(s.cpu_load_pct);
    $("cpuLoad").textContent = fmtPct(s.cpu_load_pct);
    $("cpuTop").textContent = fmtPct(s.cpu_top_core_pct);
    $("cpuMhz").textContent = fmtMhz(s.cpu_mhz_avg);
    const g = s.gpu;
    $("gpuTemp").innerHTML = g?.temp_c == null ? "—" : `${num(g.temp_c)}<small>°C</small>`;
    $("gpuBar").innerHTML = bar(g?.load_pct);
    $("gpuLoad").textContent = fmtPct(g?.load_pct);
    $("gpuMhz").textContent = fmtMhz(g?.core_mhz);
    $("gpuW").textContent = fmtW(g?.power_w);
    const pct = s.ram_total ? (100 * s.ram_used) / s.ram_total : null;
    $("ramPct").innerHTML = pct == null ? "—" : `${num(pct)}<small>%</small>`;
    $("ramBar").innerHTML = bar(pct, pct > 90);
    $("ramUsed").textContent = bytes(s.ram_used);
    $("vram").textContent = g?.vram_used == null ? "—" : `${bytes(g.vram_used)} / ${bytes(g.vram_total, 0)}`;
  });
}
