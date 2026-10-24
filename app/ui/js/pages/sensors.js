// Sensores em tempo real, agrupados por peça, com filtro por tipo e mín./máx. desde a abertura.

import { invoke, snapshot, head, esc, num, every } from "../lib.js";

const KINDS = [
  ["all", "Todos"],
  ["temperature", "Temperatura"],
  ["clock", "Clock"],
  ["load", "Carga"],
  ["voltage", "Voltagem"],
  ["fan", "Ventoinha"],
  ["power", "Consumo"],
  ["data", "Dados"],
];

const decimals = (u) => (u === "V" || u === "A" ? 3 : u === "GB" || u === "W" ? 1 : u === "°C" ? 1 : 0);
const fmt = (v, u) => `${num(v, decimals(u))} ${u}`;

export function render(el) {
  let filter = "all";
  let showThreads = false;
  el.innerHTML =
    head("RESET EXPLODE", "Sensores", `<button class="btn outline" id="reset" type="button">Zerar mín./máx.</button>`) +
    `<div class="row" style="margin-bottom:12px"><div class="chips" id="chips">${KINDS.map(([k, n]) => `<button class="chip${k === filter ? " on" : ""}" data-k="${k}" type="button">${n}</button>`).join("")}</div>
      <span class="spacer"></span><label class="row sub"><input type="checkbox" id="threads"> carga por thread</label></div>
    <div class="card" style="padding:4px 6px"><table><thead><tr><th>SENSOR</th><th>TIPO</th><th class="num">VALOR</th><th class="num">MÍN.</th><th class="num">MÁX.</th></tr></thead><tbody id="rows"></tbody></table></div>
    <p class="sub" id="hint" style="margin-top:12px"></p>`;

  const rowsEl = el.querySelector("#rows");
  el.querySelector("#chips").addEventListener("click", (e) => {
    const b = e.target.closest(".chip");
    if (!b) return;
    filter = b.dataset.k;
    el.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c === b));
    draw();
  });
  el.querySelector("#threads").addEventListener("change", (e) => {
    showThreads = e.target.checked;
    draw();
  });
  el.querySelector("#reset").addEventListener("click", () => invoke("sensors_reset"));

  let last = null;
  function draw() {
    if (!last) return;
    const kindName = Object.fromEntries(KINDS);
    const rows = last.rows.filter((r) => (filter === "all" || r.kind === filter) && (showThreads || !/^cpu\/load\/\d/.test(r.id)));
    let html = "", hw = null;
    for (const r of rows) {
      if (r.hardware !== hw) {
        hw = r.hardware;
        html += `<tr class="group"><td colspan="5">${esc(hw)}</td></tr>`;
      }
      html += `<tr><td>${esc(r.name)}</td><td class="muted">${kindName[r.kind] ?? r.kind}</td><td class="num">${fmt(r.value, r.unit)}</td><td class="num muted">${fmt(r.min, r.unit)}</td><td class="num muted">${fmt(r.max, r.unit)}</td></tr>`;
    }
    rowsEl.innerHTML = html || `<tr><td colspan="5" class="empty">Nenhum sensor deste tipo.</td></tr>`;

    // placas MSI usam o chip Nuvoton NCT6687, que o kernel não traz: sem ele não há ventoinhas nem tensões da placa-mãe
    const hasBoard = last.rows.some((r) => r.hardware.startsWith("Placa-mãe"));
    el.querySelector("#hint").innerHTML = hasBoard
      ? ""
      : "Sem sensores da placa-mãe (ventoinhas, tensões). Em placas MSI B550 eles vêm do chip NCT6687: instale <code>nct6687d-dkms-git</code> (AUR) e carregue o módulo <code>nct6687</code>.";
  }

  return every(1000, async () => {
    last = await snapshot();
    draw();
  });
}
