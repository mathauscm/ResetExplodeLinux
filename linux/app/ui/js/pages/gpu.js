// Placa de vídeo no estilo GPU-Z: ficha técnica (NVML) e sensores ao vivo.

import { invoke, snapshot, head, kv, esc, bytes, fmtTemp, fmtPct, fmtMhz, fmtW, every } from "../lib.js";

export async function render(el) {
  el.innerHTML = head("RESET EXPLODE", "Placa de vídeo") + `<div class="empty">Consultando o driver…</div>`;
  const list = await invoke("gpu_list");
  const g = list[0];
  if (!g) {
    el.innerHTML = head("RESET EXPLODE", "Placa de vídeo") +
      `<div class="card"><p>Nenhuma GPU NVIDIA encontrada pelo NVML.</p><p class="sub">Placas AMD e Intel serão lidas pelo sysfs (amdgpu / xe) numa próxima etapa.</p></div>`;
    return;
  }

  const pcie = (gen, w) => (gen && w ? `PCIe ${gen}.0 x${w}` : null);
  el.innerHTML = head("RESET EXPLODE", "Placa de vídeo") + `
    <div class="card" style="margin-bottom:12px"><div class="row"><div><div class="big" style="font-size:22px">${esc(g.name)}</div>
      <div class="sub">${esc(g.vendor)} · ${esc(g.architecture ?? "")} · driver ${esc(g.driver ?? "—")}</div></div></div></div>
    <div class="grid wide">
      <div class="card"><h2>Ficha técnica</h2>${kv([
        ["Arquitetura", g.architecture],
        ["Núcleos CUDA", g.cuda_cores],
        ["Memória", g.vram_total ? `${bytes(g.vram_total, 0)} · ${g.memory_bus_bits ?? "?"} bits` : null],
        ["Clock máx. (núcleo)", g.max_core_mhz && fmtMhz(g.max_core_mhz)],
        ["Clock máx. (memória)", g.max_mem_mhz && fmtMhz(g.max_mem_mhz)],
        ["Barramento (máximo)", pcie(g.pcie_max_gen, g.pcie_max_width)],
        ["Limite de energia", g.power_default_w ? `${fmtW(g.power_default_w)} (ajustável ${fmtW(g.power_min_w)} – ${fmtW(g.power_max_w)})` : null],
        ["Ventoinhas", g.fans],
      ])}</div>
      <div class="card"><h2>Driver e identificação</h2>${kv([
        ["Driver", g.driver],
        ["CUDA (máx.)", g.cuda],
        ["VBIOS", g.vbios],
        ["ID PCI", g.pci_device_id],
        ["Subsistema", g.pci_subsystem_id],
        ["Barramento", g.pci_bus_id],
        ["UUID", g.uuid],
      ])}</div>
      <div class="card"><h2>Ao vivo</h2><div id="live"></div><p class="sub" style="margin-top:10px">O barramento cai para PCIe 1.0 em repouso para economizar energia; em jogo ele sobe sozinho.</p></div>
    </div>`;

  const live = el.querySelector("#live");
  return every(1000, async () => {
    const l = (await snapshot()).summary.gpu;
    if (!l) return;
    live.innerHTML = kv([
      ["Temperatura", fmtTemp(l.temp_c)],
      ["Carga", fmtPct(l.load_pct)],
      ["Controlador de memória", fmtPct(l.mem_ctrl_pct)],
      ["Clock do núcleo", fmtMhz(l.core_mhz)],
      ["Clock da memória", fmtMhz(l.mem_mhz)],
      ["Consumo", l.power_w == null ? "não informado por esta placa" : `${fmtW(l.power_w)} de ${fmtW(l.power_limit_w)}${l.power_sampled ? " (média das amostras do driver)" : ""}`],
      ["Ventoinha", fmtPct(l.fan_pct)],
      ["VRAM", l.vram_used == null ? null : `${bytes(l.vram_used)} de ${bytes(l.vram_total, 0)}`],
      ["NVENC / NVDEC", `${fmtPct(l.encoder_pct)} / ${fmtPct(l.decoder_pct)}`],
      ["P-state", l.pstate],
      ["Limitado por (PerfCap)", l.perfcap.length ? l.perfcap.join(", ") : "nada"],
    ]);
  });
}
