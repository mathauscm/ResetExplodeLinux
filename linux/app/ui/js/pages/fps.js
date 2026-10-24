// FPS estimado: quanto cada jogo do catálogo deve rodar neste PC. Sem chave da HowManyFPS, a estimativa é local
// (desempenho relativo da GPU e da CPU); com chave, os números vêm de sessões reais e chegam um a um por evento.

import { call, listen, head, esc, num, bytes, bar } from "../lib.js";

const RES = [["1920x1080", "1080p"], ["2560x1440", "1440p"], ["3840x2160", "4K"]];
const PRESETS = [["Baixo", "Baixo"], ["Médio", "Médio"], ["Alto", "Alto"], ["Ultra", "Ultra"]];
const UPS = [["Native", "Nativo"], ["Quality", "Qualidade"], ["Performance", "Desempenho"]];

const rateClass = (r) => ({ EXCELENTE: "ok", "ÓTIMO": "ok", "JOGÁVEL": "", FRACO: "warn", "INJOGÁVEL": "bad" })[r] ?? "";

// as escolhas ficam salvas entre as visitas
function saved(k, def) {
  try {
    return localStorage.getItem("re.fps." + k) ?? def;
  } catch {
    return def;
  }
}
function save(k, v) {
  try {
    localStorage.setItem("re.fps." + k, v);
  } catch {
    /* sem armazenamento: só não lembra */
  }
}

export async function render(el) {
  const opt = { res: saved("res", "1920x1080"), preset: saved("preset", "Alto"), upscaling: saved("up", "Native") };
  let only = "all";
  let filter = "";
  let rows = [];
  let info = null;
  let fetching = false;

  const group = (name, label, list) => `<div class="fps-opt"><label class="field">${label}</label><div class="chips" data-group="${name}">${list
    .map(([v, l]) => `<button class="chip${opt[name] === v ? " on" : ""}" type="button" data-v="${v}">${l}</button>`)
    .join("")}</div></div>`;

  el.innerHTML = head("RESET JOGOS", "FPS estimado", `<input type="text" id="search" placeholder="Buscar jogo" class="fps-search">`) + `
    <p class="sub fps-sub" id="sub">Lendo o hardware…</p>
    <div class="card fps-opts">${group("res", "Resolução", RES)}${group("preset", "Gráficos", PRESETS)}${group("upscaling", "Upscaling (DLSS / FSR)", UPS)}</div>
    <div class="fps-top">
      <div class="card" id="hw"></div>
      <div class="card" id="source"></div>
    </div>
    <div class="row fps-bar"><div class="chips" id="only">
      <button class="chip on" type="button" data-only="all">Todos os jogos</button>
      <button class="chip" type="button" data-only="installed">Instalados neste PC</button></div>
      <span class="spacer"></span><span class="faint small">Barra: escala até 240 FPS · 144+ excelente · 90+ ótimo · 60+ jogável</span></div>
    <div class="fps-grid" id="grid"></div>`;
  const $ = (id) => el.querySelector("#" + id);

  function drawHw() {
    const m = info.machine, s = m.gpu_spec;
    $("sub").textContent = `FPS esperado no seu PC · ${m.gpu_short} · ${m.cpu_short}`;
    $("hw").innerHTML = `<h2>Seu hardware</h2>
      <div class="fps-hw"><div><div class="faint small">PLACA DE VÍDEO</div><b>${esc(m.gpu_short)}</b>
          <div class="sub">${s ? `${esc(s.chip)} · ${esc(s.arch)} · ${num(s.shaders)} shaders` : esc(m.gpu_name || "não identificada")}${m.vram_total ? ` · ${bytes(m.vram_total, 0)}` : ""}</div>
          <div class="fps-idx">${bar(Math.min(100, m.gpu_idx / 2))}<span>${num(m.gpu_idx)}${m.gpu_known ? "" : " (palpite)"}</span></div></div>
        <div><div class="faint small">PROCESSADOR</div><b>${esc(m.cpu_short)}</b><div class="sub">${esc(m.cpu_name)}</div>
          <div class="fps-idx">${bar(Math.min(100, m.cpu_idx / 2))}<span>${num(m.cpu_idx)}${m.cpu_known ? "" : " (palpite)"}</span></div></div></div>
      ${m.gpu_features ? `<div class="sub fps-feat">${esc(m.gpu_features)}</div>` : ""}
      <div class="faint small" style="margin-top:8px">Índices de desempenho em jogos: RTX 4070 = 100 e Ryzen 7 7700X = 100.</div>`;
  }

  function drawSource(msg = "") {
    const on = info.configured;
    $("source").innerHTML = `<h2>Fonte dos dados</h2>
      <b>${on ? "HowManyFPS (dados reais)" : "Estimativa local (aproximada)"}</b>
      <p class="sub" style="margin-top:4px">${on
        ? "FPS médio e 1% low medidos em sessões reais de jogo. Respostas ficam em cache por 7 dias. A API usa as configurações gráficas dela; \"Gráficos\" vale só para a estimativa."
        : "Calculada a partir do desempenho relativo da sua GPU e CPU. Para FPS medido em jogo, peça uma chave da API em info@howmanyfps.com e cole abaixo."}</p>
      <div class="row"><input type="password" id="key" placeholder="${on ? "Chave salva (cole outra para trocar)" : "Chave da API HowManyFPS"}" style="flex:1" autocomplete="off">
        <button class="btn" id="saveKey" type="button">${on ? "Atualizar chave" : "Salvar chave"}</button>
        ${on ? `<button class="btn outline" id="delKey" type="button">Remover</button>` : ""}</div>
      <div class="status" id="srcMsg">${esc(msg)}</div>`;
  }

  function card(r) {
    const low = r.low > 0 ? `1% low ${num(r.low)}` : "";
    return `<div class="card fps-card${r.installed ? " installed" : ""}" data-name="${esc(r.name)}">
      <div class="fps-head"><div class="fps-name" title="${esc(r.name)}">${esc(r.name)}</div>${r.installed ? `<span class="tag">instalado</span>` : ""}</div>
      <div class="sub">${esc(r.genre)}</div>
      <div class="fps-num"><span class="big">${r.fps > 0 ? num(r.fps) : "—"}<small>FPS</small></span>
        ${r.rating ? `<span class="fps-rate ${rateClass(r.rating)}"><i class="dot ${rateClass(r.rating)}"></i>${r.rating}</span>` : ""}</div>
      ${bar(Math.min(100, (r.fps / 240) * 100))}
      <div class="fps-foot"><span>${low}</span><span class="fps-src">${r.source}</span></div>
      <div class="faint small fps-detail" title="${esc(r.detail)}">${esc(r.detail)}</div>
    </div>`;
  }

  function drawGrid() {
    const list = rows.filter((r) => (only === "all" || r.installed) && (!filter || r.name.toLowerCase().includes(filter)));
    $("grid").innerHTML = list.map(card).join("") || `<div class="empty">${only === "installed" ? "Nenhum jogo do catálogo está instalado neste PC." : "Nada encontrado."}</div>`;
  }

  async function refresh() {
    rows = await call("games", "fps_estimate", opt);
    drawGrid();
    if (info.configured) fetchApi();
  }

  async function fetchApi() {
    fetching = true;
    $("srcMsg").textContent = "Buscando dados reais na HowManyFPS…";
    try {
      const r = await call("games", "fps_fetch", { res: opt.res, upscaling: opt.upscaling });
      if (r.cancelled) return;
      $("srcMsg").textContent = r.stopped
        ? `A API recusou a consulta (${r.stopped}). Mostrando a estimativa local.`
        : `${r.ok} de ${r.total} jogos com dados da HowManyFPS; os demais usam a estimativa local.`;
    } catch (e) {
      $("srcMsg").textContent = String(e);
    } finally {
      fetching = false;
    }
  }

  // cada resposta da API substitui a estimativa daquele jogo (e só vale se as opções ainda forem as mesmas)
  const unlisten = await listen("games-fps-row", (p) => {
    if (p.res !== opt.res || p.upscaling !== opt.upscaling) return;
    const r = rows.find((x) => x.name === p.name);
    if (!r) return;
    if (p.error) r.detail = `API: ${p.error}`;
    else Object.assign(r, { fps: p.fps, low: p.low, rating: p.rating, source: "HOWMANYFPS", detail: "" });
    const node = el.querySelector(`.fps-card[data-name="${CSS.escape(p.name)}"]`);
    if (node) node.outerHTML = card(r);
  });

  el.addEventListener("click", async (e) => {
    const chip = e.target.closest("[data-group] .chip");
    if (chip) {
      const g = chip.parentElement.dataset.group;
      opt[g] = chip.dataset.v;
      save(g === "upscaling" ? "up" : g, opt[g]);
      chip.parentElement.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c === chip));
      if (fetching) await call("games", "fps_cancel").catch(() => {});
      return refresh();
    }
    const o = e.target.closest("[data-only]");
    if (o) {
      only = o.dataset.only;
      $("only").querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c === o));
      return drawGrid();
    }
    if (e.target.id === "saveKey" || e.target.id === "delKey") {
      const key = e.target.id === "delKey" ? "" : $("key").value.trim();
      if (e.target.id === "saveKey" && !key) return ($("srcMsg").textContent = "Cole a chave antes de salvar.");
      try {
        const r = await call("games", "fps_key_set", { key });
        info.configured = r.configured;
        drawSource(r.configured ? "Chave salva (só o seu usuário consegue ler o arquivo)." : "Chave removida.");
        refresh();
      } catch (err) {
        $("srcMsg").textContent = String(err);
      }
    }
  });

  $("search").addEventListener("input", (e) => {
    filter = e.target.value.trim().toLowerCase();
    drawGrid();
  });

  info = await call("games", "fps_init");
  drawHw();
  drawSource();
  await refresh();

  return () => {
    unlisten();
    if (fetching) call("games", "fps_cancel").catch(() => {});
  };
}
