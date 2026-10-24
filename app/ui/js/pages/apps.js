// Apps: o que está instalado (com opção de remover) e um catálogo para instalar vários de uma vez
// (equivalente ao "Instalar apps" do Windows, que usava o winget). Por baixo é o pacman.

import { invoke, head, esc } from "../lib.js";
import { runJob } from "../pkgjob.js";

const avatar = (a) => (a.icon ? `<img class="app-icon" src="${a.icon}" alt="">` : `<div class="app-icon letter">${esc(a.name[0] ?? "?")}</div>`);

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Apps", `<div class="chips" id="tabs">
      <button class="chip on" data-tab="installed" type="button">Instalados</button>
      <button class="chip" data-tab="catalog" type="button">Instalar</button></div>`) + `
    <div class="card job" id="job" hidden style="margin-bottom:12px"></div>
    <div id="body"></div>`;

  const $ = (id) => el.querySelector("#" + id);
  let tab = "installed";
  let apps = null;
  let catalog = null;
  let filter = "";
  let confirming = null; // pacote esperando o segundo clique em "Remover"
  const picked = new Set();
  let busy = false;

  // ---------------- instalados ----------------
  function drawInstalled() {
    const list = apps.filter((a) => !filter || `${a.name} ${a.package} ${a.comment}`.toLowerCase().includes(filter));
    $("body").innerHTML = `<div class="row" style="margin-bottom:12px"><input type="text" id="search" placeholder="Buscar nos ${apps.length} apps instalados" value="${esc(filter)}" style="flex:1">
        <button class="btn outline" id="reload" type="button">Atualizar lista</button></div>
      <div class="card" style="padding:6px 14px">${list
        .map((a) => `<div class="app-row">${avatar(a)}
          <div style="flex:1;min-width:0"><b>${esc(a.name)}</b>${a.also.length ? ` <span class="faint">+ ${esc(a.also.join(", "))}</span>` : ""}
            <div class="sub ellipsis">${esc(a.comment)}</div>
            <div class="faint mono">${esc(a.package || "—")}${a.version ? ` · ${esc(a.version)}` : ""} · ${esc(a.source)}${a.size ? ` · ${esc(a.size)}` : ""}</div></div>
          ${a.locked
            ? `<span class="faint small" title="${esc(a.locked)}">${esc(a.locked.length > 40 ? a.locked.slice(0, 40) + "…" : a.locked)}</span>`
            : `<button class="btn ${confirming === a.package ? "danger" : "outline"}" type="button" data-remove="${esc(a.package)}" ${busy ? "disabled" : ""}>${confirming === a.package ? "Confirmar remoção" : "Remover"}</button>`}
        </div>`)
        .join("") || `<div class="empty">Nada encontrado.</div>`}</div>`;
    const s = $("search");
    s.addEventListener("input", () => {
      filter = s.value.trim().toLowerCase();
      const pos = s.selectionStart;
      drawInstalled();
      $("search").focus();
      $("search").setSelectionRange(pos, pos);
    });
    $("reload").addEventListener("click", loadInstalled);
  }

  async function loadInstalled() {
    $("body").innerHTML = `<div class="empty">Lendo os apps instalados…</div>`;
    apps = await invoke("apps_installed");
    if (tab === "installed") drawInstalled();
  }

  // ---------------- catálogo ----------------
  function drawCatalog() {
    const cats = [...new Set(catalog.map((c) => c.category))];
    $("body").innerHTML = cats
      .map((cat) => `<h2 style="margin-top:6px">${esc(cat)}</h2><div class="grid" style="margin-bottom:16px">${catalog
        .filter((c) => c.category === cat)
        .map((c) => `<label class="card cat-item${c.installed ? " done" : ""}">
            <input type="checkbox" data-pkg="${esc(c.package)}" ${c.installed ? "disabled" : picked.has(c.package) ? "checked" : ""}>
            ${avatar(c)}
            <div><b>${esc(c.name)}</b> ${c.installed ? '<span class="tag">instalado</span>' : ""}
              <div class="sub">${esc(c.description)}</div>
              ${c.replaces ? `<div class="faint small">no lugar do ${esc(c.replaces)}</div>` : ""}</div></label>`)
        .join("")}</div>`)
      .join("") + `
      <div class="install-bar"><button class="btn" id="install" type="button" ${picked.size && !busy ? "" : "disabled"}>Instalar selecionados (${picked.size})</button>
        <span class="sub">Instalar também aplica as atualizações pendentes do sistema: o Arch não suporta instalar com a lista de pacotes desatualizada.</span></div>`;
  }

  async function loadCatalog() {
    catalog = await invoke("apps_catalog");
    if (tab === "catalog") drawCatalog();
  }

  // ---------------- eventos ----------------
  $("tabs").addEventListener("click", (e) => {
    const b = e.target.closest("[data-tab]");
    if (!b) return;
    tab = b.dataset.tab;
    el.querySelectorAll("#tabs .chip").forEach((c) => c.classList.toggle("on", c === b));
    if (tab === "installed") apps ? drawInstalled() : loadInstalled();
    else catalog ? drawCatalog() : loadCatalog();
  });

  $("body").addEventListener("change", (e) => {
    const p = e.target.dataset.pkg;
    if (!p) return;
    e.target.checked ? picked.add(p) : picked.delete(p);
    drawCatalog();
  });

  $("body").addEventListener("click", async (e) => {
    const rm = e.target.closest("[data-remove]");
    if (rm) {
      const pkg = rm.dataset.remove;
      if (confirming !== pkg) {
        // primeiro clique só arma; o segundo confirma (sem diálogo do navegador, que o Tauri bloqueia)
        confirming = pkg;
        drawInstalled();
        return;
      }
      confirming = null;
      busy = true;
      drawInstalled();
      const name = apps.find((a) => a.package === pkg)?.name ?? pkg;
      await runJob({ kind: "remove", packages: [pkg] }, $("job"), `Removendo ${name}`);
      busy = false;
      catalog = null;
      await loadInstalled();
      return;
    }
    if (e.target.id === "install") {
      busy = true;
      drawCatalog();
      const pkgs = [...picked];
      const r = await runJob({ kind: "install", packages: pkgs }, $("job"), `Instalando ${pkgs.length} app(s)`);
      busy = false;
      if (r.ok) picked.clear();
      apps = null;
      await loadCatalog();
    }
  });

  await loadInstalled();
}
