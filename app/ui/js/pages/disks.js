// Discos físicos e as unidades montadas de cada um, com anel de cheio/vazio (como o DriveMap do Windows).

import { invoke, head, esc, bytes, num } from "../lib.js";

function ring(pct) {
  const r = 24, c = 2 * Math.PI * r;
  const p = Math.max(0, Math.min(100, pct));
  return `<svg class="ring" viewBox="0 0 54 54"><circle class="track" cx="27" cy="27" r="${r}"/>
    <circle class="fill" cx="27" cy="27" r="${r}" stroke-dasharray="${(c * p) / 100} ${c}" style="${p > 90 ? "stroke:var(--bad)" : ""}"/>
    <text x="27" y="27">${num(p)}%</text></svg>`;
}

export async function render(el) {
  el.innerHTML = head("RESET EXPLODE", "Discos", `<button class="btn outline" id="refresh" type="button">Atualizar</button>`) + `<div id="list"></div>`;
  const list = el.querySelector("#list");

  async function load() {
    const disks = await invoke("disks_list");
    list.innerHTML = disks.length
      ? `<div class="grid wide">${disks.map((d) => `
        <div class="card">
          <div class="row"><b>${esc(d.model)}</b><span class="spacer"></span><span class="tag">${esc(d.kind)}</span>${d.removable ? '<span class="tag">removível</span>' : ""}</div>
          <div class="sub" style="margin-bottom:8px">/dev/${esc(d.name)} · ${bytes(d.size)}${d.serial ? ` · série ${esc(d.serial)}` : ""}</div>
          ${d.volumes.length
            ? d.volumes.map((v) => {
                const used = v.total - v.free;
                return `<div class="vol">${ring(v.total ? (100 * used) / v.total : 0)}<div>
                  <b>${esc(v.label || v.mount)}</b> <span class="tag">${esc(v.fs)}</span>
                  <div class="sub">${esc(v.mount)} · ${esc(v.device)}</div>
                  <div class="sub">${bytes(used)} usados · ${bytes(v.free)} livres de ${bytes(v.total)}</div></div></div>`;
              }).join("")
            : `<div class="vol sub">Nenhuma partição montada.</div>`}
        </div>`).join("")}</div>`
      : `<div class="empty">Nenhum disco encontrado.</div>`;
  }

  el.querySelector("#refresh").addEventListener("click", load);
  await load();
}
