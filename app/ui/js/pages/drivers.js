// Drivers e atualizações: o que está em uso (NVIDIA, kernel, Mesa, firmware) e as atualizações pendentes,
// com escolha do que atualizar. Por baixo é o pacman; a senha é pedida pela janela do sistema.

import { invoke, head, esc } from "../lib.js";
import { runJob } from "../pkgjob.js";

const ORDER = ["Placa de vídeo (NVIDIA)", "Kernel", "Gráficos (Mesa e Vulkan)", "Firmware e microcódigo", "Outros pacotes do sistema"];
const NEEDS_REBOOT = ["Placa de vídeo (NVIDIA)", "Kernel", "Firmware e microcódigo"];

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Drivers e atualizações", `<button class="btn" id="check" type="button">Buscar atualizações</button>`) + `
    <div class="grid wide">
      <div class="card"><h2>Em uso agora</h2><div id="overview"><div class="sub">Lendo…</div></div></div>
      <div class="card"><h2>Atualizações</h2><div id="updates"><div class="sub">Buscando atualizações…</div></div></div>
    </div>
    <div class="card job" id="job" hidden style="margin-top:12px"></div>
    <details class="card" style="margin-top:12px"><summary class="sub" style="cursor:pointer">Pacotes de driver instalados</summary><div id="pkgs" style="margin-top:10px"></div></details>`;

  const $ = (id) => el.querySelector("#" + id);
  let updates = [];
  let checked = new Set();
  let busy = false;

  async function loadOverview() {
    const o = await invoke("drivers_overview");
    const row = (k, v) => (v ? `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>` : "");
    $("overview").innerHTML = `<dl class="kv">
        ${row("Placa de vídeo", o.gpu)}
        ${row("Driver NVIDIA", o.nvidia_driver && `${o.nvidia_driver}${o.nvidia_module ? ` · módulo ${o.nvidia_module}` : ""}`)}
        ${row("Kernel em uso", o.kernel_running)}
        ${row("Kernels instalados", o.kernels.map(([n, v]) => `${n} ${v}`).join(" · "))}
        ${row("Mesa", o.mesa)}
        ${row("Firmware", o.firmware)}
        ${row("Microcódigo da CPU", o.microcode)}
      </dl>
      ${o.reboot_needed ? `<div class="test-banner">Há um kernel mais novo instalado do que o que está rodando. <b>Reinicie o PC</b> para usá-lo.</div>` : ""}`;
    $("pkgs").innerHTML = `<table><thead><tr><th>CATEGORIA</th><th>PACOTE</th><th class="num">VERSÃO</th></tr></thead><tbody>${o.packages
      .map(([c, n, v]) => `<tr><td class="muted">${esc(c)}</td><td>${esc(n)}</td><td class="num">${esc(v)}</td></tr>`)
      .join("")}</tbody></table>`;
  }

  function drawUpdates() {
    if (!updates.length) {
      $("updates").innerHTML = `<div class="check"><span class="dot ok"></span><div><b>Tudo em dia</b><div class="sub">Nenhuma atualização pendente.</div></div></div>`;
      return;
    }
    const groupSize = (g) => updates.filter((u) => u.group === g).length;
    const cats = ORDER.filter((c) => updates.some((u) => u.category === c));
    const skipped = updates.filter((u) => !checked.has(u.name));
    $("updates").innerHTML = cats
      .map((c) => `<div class="upd-cat"><div class="row"><b>${esc(c)}</b><span class="spacer"></span>
          <button class="link" type="button" data-all="${esc(c)}">marcar todos</button></div>
          ${updates.filter((u) => u.category === c).map((u) => `<label class="upd"><input type="checkbox" data-name="${esc(u.name)}" ${checked.has(u.name) ? "checked" : ""}>
            <span class="upd-name">${esc(u.name)}</span><span class="faint">${esc(u.from)} → <b class="fg">${esc(u.to)}</b></span>
            ${groupSize(u.group) > 1 ? `<span class="tag" title="Precisam ir na mesma versão: marcar ou desmarcar um muda todos">vai junto com mais ${groupSize(u.group) - 1}</span>` : ""}</label>`).join("")}
        </div>`)
      .join("") + `
      ${skipped.length ? `<div class="test-banner">No Arch, deixar pacotes para trás pode quebrar o sistema (versões que não combinam). O recomendado é atualizar tudo; desmarque só se souber por quê.</div>` : ""}
      <div class="row" style="margin-top:14px"><button class="btn" id="apply" type="button" ${checked.size && !busy ? "" : "disabled"}>Atualizar ${checked.size === updates.length ? "tudo" : "selecionados"} (${checked.size})</button>
        <span class="sub">Pede a senha de administrador.</span></div>`;
  }

  async function check() {
    $("check").disabled = true;
    $("updates").innerHTML = `<div class="sub">Buscando atualizações nos servidores…</div>`;
    try {
      updates = await invoke("updates_check");
      checked = new Set(updates.map((u) => u.name));
      drawUpdates();
    } catch (e) {
      $("updates").innerHTML = `<div class="check"><span class="dot bad"></span><div><b>Não deu para buscar</b><div class="sub">${esc(String(e))}</div></div></div>`;
    } finally {
      $("check").disabled = busy;
    }
  }

  $("updates").addEventListener("change", (e) => {
    const name = e.target.dataset.name;
    if (!name) return;
    const g = updates.find((u) => u.name === name).group;
    for (const u of updates.filter((x) => x.group === g)) e.target.checked ? checked.add(u.name) : checked.delete(u.name);
    drawUpdates();
  });

  $("updates").addEventListener("click", async (e) => {
    const all = e.target.closest("[data-all]");
    if (all) {
      for (const u of updates.filter((x) => x.category === all.dataset.all)) checked.add(u.name);
      for (const u of updates.filter((x) => checked.has(x.name))) for (const v of updates.filter((y) => y.group === u.group)) checked.add(v.name);
      drawUpdates();
      return;
    }
    if (e.target.id !== "apply") return;
    busy = true;
    $("check").disabled = true;
    drawUpdates();
    const done = updates.filter((u) => checked.has(u.name));
    const ignore = updates.filter((u) => !checked.has(u.name)).map((u) => u.name);
    const r = await runJob({ kind: "upgrade", ignore }, $("job"), `Atualizando ${done.length} pacote(s)`);
    busy = false;
    if (r.ok && done.some((u) => NEEDS_REBOOT.includes(u.category)))
      $("job").insertAdjacentHTML("beforeend", `<div class="test-banner"><b>Reinicie o PC</b> para usar o kernel/driver novo.</div>`);
    await Promise.all([check(), loadOverview()]);
  });

  $("check").addEventListener("click", check);
  loadOverview();
  await check();
}
